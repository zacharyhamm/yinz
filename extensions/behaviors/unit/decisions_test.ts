import {
  assertCurrentHandoff,
  type DecisionAction,
  resolveDecisionArgs,
} from "../_lib/decisions.ts";
import { type Handoff, HandoffSchema } from "../_lib/issue_driver.ts";
import {
  codeReviewFingerprint,
  createState,
  type Issue,
  type LifecycleState,
  reviewFingerprint,
  StateSchema,
  transition,
} from "../_lib/lifecycle.ts";

function assert(
  condition: unknown,
  message = "Assertion failed",
): asserts condition {
  if (!condition) throw new Error(message);
}
function equal(actual: unknown, expected: unknown): void {
  assert(
    JSON.stringify(actual) === JSON.stringify(expected),
    `${JSON.stringify(actual)} != ${JSON.stringify(expected)}`,
  );
}
function rejects(fn: () => unknown, message: string): void {
  try {
    fn();
  } catch (error) {
    assert(
      error instanceof Error && error.message.includes(message),
      String(error),
    );
    return;
  }
  throw new Error(`Expected rejection containing: ${message}`);
}

const head = "a".repeat(40);
const handoffId = "0b9c7a2e-4c1d-4f7e-9a3b-6d5e4f3a2b1c";
const otherId = "1c9c7a2e-4c1d-4f7e-9a3b-6d5e4f3a2b1c";
const issue: Issue = {
  workspace: "default",
  itemId: 85,
  text: "Fix issue",
  note: "Acceptance criteria",
  digest: "c".repeat(64),
};
const plan = {
  summary: "Fix the behavior",
  steps: [{ id: "fix", description: "Fix it", files: ["src/lib.rs"] }],
  testingStrategy: "Run the affected crate tests",
  checks: [{
    id: "crate",
    command: ["cargo", "x", "test", "-p", "demo"],
    covers: ["fix"],
  }],
};

function planning(): LifecycleState {
  let state = transition(createState(issue), "triage", {
    classification: "bug",
    summary: "Confirmed reproduction",
  });
  state = transition(state, "plan", plan);
  return transition(state, "review_plan", { planVersion: 1, findings: [] });
}
function reviewed(): LifecycleState {
  let state = transition(planning(), "approve", {
    planVersion: 1,
    request: "Implement this plan",
  });
  state = transition(state, "implement", {
    worktree: "/tmp/issue-85",
    branch: "issue-85",
  });
  state = transition(state, "review_code", {
    planVersion: 1,
    commit: head,
    steps: [{ id: "fix", status: "implemented" }],
    findings: [],
  });
  return transition(state, "review_ux", {
    planVersion: 1,
    commit: head,
    assessment: {
      impact: "no_material_impact",
      summary: "Fixture behavior is unchanged.",
      assessedPaths: ["src/lib.rs: caller"],
      limitations: ["Source-based fixture assessment"],
    },
    findings: [],
  });
}
function withFindings(): LifecycleState {
  const state = reviewed();
  state.codeReview!.findings = [
    { id: "race", severity: "high", description: "Lost work" },
    { id: "leak", severity: "low", description: "Handle leak" },
  ];
  state.uxReview!.findings = [
    { id: "copy", severity: "low", description: "Unclear message" },
  ];
  return StateSchema.parse(state);
}
function verified(): LifecycleState {
  const state = reviewed();
  const id = "b6da10b0-c980-4b82-b920-640238095f1a";
  return transition(state, "verify", {
    verification: {
      id,
      planVersion: 1,
      commit: head,
      branch: "issue-85",
      startedAt: "2026-11-03T11:00:00.000Z",
      finishedAt: "2026-11-03T11:05:00.000Z",
      status: "passed",
      checks: plan.checks.map((check) => ({
        ...check,
        exitCode: 0,
        durationMs: 10,
        log: `check-log-${id}-${check.id}`,
        worktreeUnchanged: true,
        status: "passed",
      })),
    },
  });
}

/** A handoff as drive records it for this exact state. */
function handoff(
  state: LifecycleState,
  decision: Handoff["decision"],
  patch: Partial<Handoff> = {},
): Handoff {
  return HandoffSchema.parse({
    id: handoffId,
    status: "current",
    decision,
    reason: "Fixture",
    createdAt: "2026-11-03T12:00:00.000Z",
    state,
    ...(decision !== "blocked" && state.codeReview && state.uxReview
      ? { codeReviewFingerprint: codeReviewFingerprint(state) }
      : {}),
    ...(state.planReview
      ? { reviewFingerprint: reviewFingerprint(state) }
      : {}),
    ...(state.plan ? { planVersion: state.plan.version } : {}),
    ...(state.codeReview ? { commit: state.codeReview.commit } : {}),
    presentation: "Fixture",
    ...patch,
  });
}
function resolve(
  action: DecisionAction,
  raw: unknown,
  state: LifecycleState,
  current: Handoff | undefined,
) {
  return resolveDecisionArgs(action, raw, state, () => current);
}

Deno.test("approve derives planVersion from the current approve_plan handoff", () => {
  const state = planning();
  const request = "Approve plan version 1 and its checks";
  const args = resolve(
    "approve",
    { handoffId, request, workspace: "default", itemId: 85 },
    state,
    handoff(state, "approve_plan"),
  );
  equal(args, { planVersion: 1, request });
  equal(transition(state, "approve", args).approval, {
    planVersion: 1,
    request,
  });
});

Deno.test("approve rejects missing, invalidated, other-type, other-id and state-mismatched handoffs", () => {
  const state = planning();
  const request = "Approve";
  const changed = structuredClone(state);
  changed.plan!.summary = "Changed after presentation";
  for (
    const current of [
      undefined,
      handoff(state, "approve_plan", { status: "invalidated" }),
      handoff(state, "authorize_plan_revision"),
      handoff(state, "approve_plan", { id: otherId }),
      handoff(changed, "approve_plan"),
    ]
  ) {
    rejects(
      () => resolve("approve", { handoffId, request }, state, current),
      "stale",
    );
  }
  rejects(
    () =>
      resolve(
        "approve",
        { handoffId, planVersion: 2, request },
        state,
        handoff(state, "approve_plan"),
      ),
    "stale",
  );
  equal(
    resolve(
      "approve",
      { handoffId, planVersion: 1, request },
      state,
      handoff(state, "approve_plan"),
    ),
    { planVersion: 1, request },
  );
});

Deno.test("legacy approve keeps planVersion binding without reading the handoff", () => {
  const state = planning();
  let reads = 0;
  const read = () => {
    reads++;
    throw new Error("legacy inputs must not read the handoff");
  };
  equal(
    resolveDecisionArgs(
      "approve",
      { planVersion: 1, request: "Go" },
      state,
      read,
    ),
    { planVersion: 1, request: "Go" },
  );
  equal(
    resolveDecisionArgs(
      "ready",
      { commit: head, mergeRequest: " Merge " },
      state,
      read,
    ),
    { commit: head, mergeRequest: "Merge" },
  );
  equal(
    resolveDecisionArgs(
      "adjudicate_plan",
      {
        planVersion: 1,
        reviewFingerprint: "f".repeat(64),
        dispositions: [{
          findingId: "gap",
          disposition: "resolved",
          rationale: "Covered",
          evidence: ["a.ts:1"],
        }],
      },
      state,
      read,
    ),
    {
      planVersion: 1,
      reviewFingerprint: "f".repeat(64),
      dispositions: [{
        findingId: "gap",
        disposition: "resolved",
        rationale: "Covered",
        evidence: ["a.ts:1"],
      }],
    },
  );
  equal(reads, 0);
  rejects(
    () => resolveDecisionArgs("approve", { request: "Go" }, state, read),
    "handoffId",
  );
  rejects(
    () => resolveDecisionArgs("ready", { mergeRequest: "Merge" }, state, read),
    "handoffId",
  );
  rejects(
    () =>
      resolveDecisionArgs(
        "adjudicate_plan",
        {
          planVersion: 1,
          dispositions: [{
            findingId: "gap",
            disposition: "resolved",
            rationale: "Covered",
            evidence: ["a.ts:1"],
          }],
        },
        state,
        read,
      ),
    "handoffId",
  );
  equal(reads, 0);
});

Deno.test("adjudicate-plan derives planVersion and fingerprint from authorize_plan_revision or approve_plan", () => {
  const state = planning();
  const dispositions = [{
    findingId: "gap",
    disposition: "resolved",
    rationale: "Covered by the existing caller test",
    evidence: ["src/lib.rs:42"],
  }];
  for (const decision of ["authorize_plan_revision", "approve_plan"] as const) {
    equal(
      resolve(
        "adjudicate_plan",
        { handoffId, dispositions },
        state,
        handoff(state, decision),
      ),
      {
        planVersion: 1,
        reviewFingerprint: reviewFingerprint(state),
        dispositions,
      },
    );
  }
  rejects(
    () =>
      resolve(
        "adjudicate_plan",
        { handoffId, dispositions },
        state,
        handoff(state, "authorize_merge"),
      ),
    "stale",
  );
  rejects(
    () =>
      resolve(
        "adjudicate_plan",
        { handoffId, reviewFingerprint: "0".repeat(64), dispositions },
        state,
        handoff(state, "approve_plan"),
      ),
    "stale",
  );
});

Deno.test("decide-review binds id to the handoff and derives codeReviewFingerprint", () => {
  const state = withFindings();
  const current = handoff(state, "address_review_findings");
  const request = "Repair the race; ignore the rest";
  const repair = [{ role: "review_code", findingId: "race" }];
  const ignore = [
    { role: "review_code", findingId: "leak" },
    { role: "review_ux", findingId: "copy" },
  ];
  const expected = {
    id: handoffId,
    reviewFingerprint: codeReviewFingerprint(state),
    request,
    repair,
    ignore,
  };
  equal(
    resolve(
      "decide_review",
      { handoffId, request, repair, ignore },
      state,
      current,
    ),
    expected,
  );
  // The legacy form names the handoff as the decision id.
  equal(
    resolve(
      "decide_review",
      {
        id: handoffId,
        reviewFingerprint: codeReviewFingerprint(state),
        request,
        repair,
        ignore,
      },
      state,
      current,
    ),
    expected,
  );
  const decided = transition(
    state,
    "decide_review",
    resolve(
      "decide_review",
      { handoffId, request, repair, ignore },
      state,
      current,
    ),
  );
  equal(decided.reviewDecisions!.at(-1)!.id, handoffId);
  rejects(
    () =>
      resolve(
        "decide_review",
        { handoffId, id: otherId, request, repair },
        state,
        current,
      ),
    "differs",
  );
  rejects(
    () => resolve("decide_review", { request, repair }, state, current),
    "handoffId",
  );
  rejects(
    () =>
      resolve(
        "decide_review",
        { handoffId, reviewFingerprint: "0".repeat(64), request, repair },
        state,
        current,
      ),
    "stale",
  );
  rejects(
    () =>
      resolve(
        "decide_review",
        { id: otherId, request, repair },
        state,
        current,
      ),
    "stale",
  );
  rejects(
    () =>
      resolve(
        "decide_review",
        { handoffId, request, repair },
        state,
        handoff(state, "address_review_findings", {
          codeReviewFingerprint: "0".repeat(64),
        }),
      ),
    "Review findings changed",
  );
});

Deno.test("ignoreRemaining expands to every outstanding finding not selected for repair", () => {
  const state = withFindings();
  const current = handoff(state, "address_review_findings");
  equal(
    resolve(
      "decide_review",
      {
        handoffId,
        request: "Repair the race and ignore everything else",
        repair: [{ role: "review_code", findingId: "race" }],
        ignore: [{ role: "review_ux", findingId: "copy" }],
        ignoreRemaining: true,
      },
      state,
      current,
    ),
    {
      id: handoffId,
      reviewFingerprint: codeReviewFingerprint(state),
      request: "Repair the race and ignore everything else",
      repair: [{ role: "review_code", findingId: "race" }],
      ignore: [
        { role: "review_ux", findingId: "copy" },
        { role: "review_code", findingId: "leak" },
      ],
    },
  );
  const all = resolve(
    "decide_review",
    {
      handoffId,
      request: "Merge with every finding outstanding",
      ignoreRemaining: true,
    },
    state,
    current,
  ) as { repair: unknown[]; ignore: unknown[] };
  equal(all.repair, []);
  equal(all.ignore, [
    { role: "review_code", findingId: "race" },
    { role: "review_code", findingId: "leak" },
    { role: "review_ux", findingId: "copy" },
  ]);
});

Deno.test("approve-review-fixes derives the active repair id", () => {
  const reviewedState = withFindings();
  let state = transition(reviewedState, "decide_review", {
    id: otherId,
    reviewFingerprint: codeReviewFingerprint(reviewedState),
    request: "Repair the race",
    repair: [{ role: "review_code", findingId: "race" }],
    ignore: [
      { role: "review_code", findingId: "leak" },
      { role: "review_ux", findingId: "copy" },
    ],
  });
  state = transition(state, "plan_review_fixes", plan);
  const current = handoff(state, "approve_review_fixes");
  const request = "Implement this repair plan";
  equal(
    resolve("approve_review_fixes", { handoffId, request }, state, current),
    { id: otherId, handoffId, request },
  );
  equal(
    resolve(
      "approve_review_fixes",
      { handoffId, id: otherId, request },
      state,
      current,
    ),
    { id: otherId, handoffId, request },
  );
  rejects(
    () =>
      resolve(
        "approve_review_fixes",
        { handoffId, id: handoffId, request },
        state,
        current,
      ),
    "stale",
  );
  rejects(
    () =>
      resolve(
        "approve_review_fixes",
        { handoffId, request },
        state,
        handoff(state, "address_review_findings"),
      ),
    "stale",
  );
  const done = verified();
  rejects(
    () =>
      resolve(
        "approve_review_fixes",
        { handoffId, request },
        done,
        handoff(done, "approve_review_fixes"),
      ),
    "current review repair plan",
  );
});

Deno.test("ready derives commit from authorize_merge only", () => {
  const state = verified();
  const mergeRequest = "Merge this reviewed commit";
  equal(
    resolve(
      "ready",
      { handoffId, mergeRequest },
      state,
      handoff(state, "authorize_merge"),
    ),
    { commit: head, mergeRequest },
  );
  rejects(
    () =>
      resolve(
        "ready",
        { handoffId, commit: "b".repeat(40), mergeRequest },
        state,
        handoff(state, "authorize_merge"),
      ),
    "stale",
  );
  for (const decision of ["address_review_findings", "integrate"] as const) {
    rejects(
      () =>
        resolve(
          "ready",
          { handoffId, mergeRequest },
          state,
          handoff(state, decision),
        ),
      "stale",
    );
  }
  // The legacy path is unchanged: it binds only the explicit commit.
  equal(
    resolve("ready", { commit: head, mergeRequest }, state, undefined),
    { commit: head, mergeRequest },
  );
});

Deno.test("assertCurrentHandoff returns the exact current handoff", () => {
  const state = planning();
  const current = handoff(state, "approve_plan");
  equal(
    assertCurrentHandoff(current, state, handoffId, ["approve_plan"]),
    current,
  );
});
