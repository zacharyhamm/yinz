import {
  assertCurrentHandoff,
  type DecisionAction,
  resolveDecisionArgs,
} from "../_lib/decisions.ts";
import { type Handoff, HandoffSchema } from "../_lib/issue_driver.ts";
import { nextStep } from "../_lib/handoff_next.ts";
import {
  codeReviewFingerprint,
  createState,
  deferredPlanFindings,
  effectivePlanFindings,
  type Issue,
  type LifecycleState,
  reviewFingerprint,
  rollbackState,
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
    ...(decision !== "blocked" && state.codeReview
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

Deno.test("ignoreRemaining expands to every blocking finding not selected for repair", () => {
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
      // Low findings are deferred follow-ups, never expanded into ignores.
      ignore: [{ role: "review_ux", findingId: "copy" }],
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
  equal(all.ignore, [{ role: "review_code", findingId: "race" }]);
});

Deno.test("decide-review binds a code-stage handoff before UX review runs", () => {
  const state = withFindings();
  delete state.uxReview;
  const current = handoff(state, "address_review_findings");
  equal(current.codeReviewFingerprint, codeReviewFingerprint(state));
  const decision = resolve(
    "decide_review",
    {
      handoffId,
      request: "Repair the race",
      repair: [{ role: "review_code", findingId: "race" }],
      ignoreRemaining: true,
    },
    state,
    current,
  );
  equal(decision, {
    id: handoffId,
    reviewFingerprint: codeReviewFingerprint(state),
    request: "Repair the race",
    repair: [{ role: "review_code", findingId: "race" }],
    ignore: [],
  });
  const next = transition(state, "decide_review", decision);
  equal(next.reviewDecisions![0].uxReview, undefined);
  // UX findings are not selectable before UX review runs.
  rejects(
    () =>
      transition(state, "decide_review", {
        ...(decision as object),
        repair: [{ role: "review_ux", findingId: "copy" }],
      }),
    "Unknown or resolved review finding",
  );
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

/** A reviewed plan whose high findings the automatic revision did not settle. */
function blockedPlanning(): LifecycleState {
  let state = transition(createState(issue), "triage", {
    classification: "bug",
    summary: "Confirmed reproduction",
  });
  state = transition(state, "plan", plan);
  return transition(state, "review_plan", {
    planVersion: 1,
    findings: [
      {
        id: "gap",
        severity: "high",
        description: "Caller bypasses validation",
        requirement: "Validate every caller",
        mechanism: "The legacy caller skips the new check",
        evidence: ["src/lib.rs:42"],
      },
      {
        id: "scope",
        severity: "critical",
        description: "Migration is unplanned",
        requirement: "Existing records keep loading",
        mechanism: "Old records fail to parse",
        evidence: ["src/store.rs:7"],
      },
      { id: "naming", severity: "medium", description: "Unclear name" },
    ],
  });
}
const deferral = "Plan v1 is sufficient; defer gap and scope to code review";

Deno.test("plan approval with unresolved blocking findings is refused by default", () => {
  const state = blockedPlanning();
  // A bare approval at the revision handoff names the explicit deferral it needs.
  rejects(
    () =>
      resolve(
        "approve",
        { handoffId, request: "Implement it" },
        state,
        handoff(state, "authorize_plan_revision"),
      ),
    "--ignore review_plan/<id>",
  );
  // The transition refuses an approval without a deferral of every blocker.
  rejects(
    () =>
      transition(state, "approve", { planVersion: 1, request: "Implement" }),
    "Resolve high/critical findings first: gap, scope",
  );
  rejects(
    () =>
      transition(state, "approve", {
        planVersion: 1,
        request: deferral,
        deferFindings: {
          handoffId,
          reviewFingerprint: reviewFingerprint(state),
          findingIds: ["gap"],
        },
      }),
    "Resolve high/critical findings first: scope",
  );
});

Deno.test("deferring plan findings needs the user's explicit request and the exact handoff", () => {
  const state = blockedPlanning();
  const current = handoff(state, "authorize_plan_revision");
  for (const request of [undefined, "", "   "]) {
    rejects(
      () =>
        resolve(
          "approve",
          { handoffId, request, ignoreRemaining: true },
          state,
          current,
        ),
      "request",
    );
  }
  // Legacy explicit inputs cannot defer findings: there is no handoff to bind.
  rejects(
    () =>
      resolve(
        "approve",
        { planVersion: 1, request: deferral, ignore: ["gap", "scope"] },
        state,
        undefined,
      ),
    "requires handoffId",
  );
  const changed = structuredClone(state);
  changed.planReview!.findings[0].description = "Changed after presentation";
  for (
    const stale of [
      undefined,
      handoff(state, "authorize_plan_revision", { status: "invalidated" }),
      handoff(state, "authorize_plan_revision", { id: otherId }),
      handoff(changed, "authorize_plan_revision"),
      handoff(state, "address_review_findings"),
    ]
  ) {
    rejects(
      () =>
        resolve(
          "approve",
          { handoffId, request: deferral, ignoreRemaining: true },
          state,
          stale,
        ),
      "stale",
    );
  }
  rejects(
    () =>
      resolve(
        "approve",
        { handoffId, request: deferral, ignoreRemaining: true },
        state,
        handoff(state, "authorize_plan_revision", {
          reviewFingerprint: "f".repeat(64),
        }),
      ),
    "Plan review changed",
  );
  rejects(
    () =>
      resolve(
        "approve",
        { handoffId, planVersion: 2, request: deferral, ignoreRemaining: true },
        state,
        current,
      ),
    "stale",
  );
  // A deferral bound to another review fingerprint is refused by the transition too.
  rejects(
    () =>
      transition(state, "approve", {
        planVersion: 1,
        request: deferral,
        deferFindings: {
          handoffId,
          reviewFingerprint: "f".repeat(64),
          findingIds: ["gap", "scope"],
        },
      }),
    "stale",
  );
  for (const ignore of [["unknown", "gap", "scope"], ["gap", "gap", "scope"]]) {
    rejects(
      () =>
        transition(
          state,
          "approve",
          resolve(
            "approve",
            { handoffId, request: deferral, ignore },
            state,
            current,
          ),
        ),
      ignore.includes("unknown")
        ? "Unknown or resolved plan finding"
        : "unique",
    );
  }
});

Deno.test("the user can approve a plan despite findings, which stay unresolved and visible", () => {
  const state = blockedPlanning();
  const current = handoff(state, "authorize_plan_revision");
  const guidance = nextStep(current, state);
  assert(guidance.includes(`approve --handoff ${handoffId}`), guidance);
  assert(guidance.includes("review_plan/gap, review_plan/scope"), guidance);
  const args = resolve(
    "approve",
    { handoffId, request: deferral, ignore: ["naming"], ignoreRemaining: true },
    state,
    current,
  );
  equal(args, {
    planVersion: 1,
    request: deferral,
    deferFindings: {
      handoffId,
      reviewFingerprint: reviewFingerprint(state),
      findingIds: ["naming", "gap", "scope"],
    },
  });
  const approved = transition(state, "approve", args);
  equal(approved.phase, "approved");
  // The original review and its effective findings are untouched.
  equal(approved.planReview, state.planReview);
  assert(effectivePlanFindings(approved).every((f) => !f.resolution));
  const deferred = deferredPlanFindings(approved)!;
  equal(deferred.handoffId, handoffId);
  equal(deferred.reviewFingerprint, reviewFingerprint(state));
  equal(deferred.findings, [
    state.planReview!.findings[0],
    state.planReview!.findings[1],
    state.planReview!.findings[2],
  ]);
  equal(approved.approval!.request, deferral);
  // The deferral is approval evidence: later stages keep it, and rolling back
  // the approval revokes it together with the approval.
  const implemented = transition(approved, "implement", {
    worktree: "/tmp/issue-85",
    branch: "issue-85",
  });
  equal(deferredPlanFindings(implemented), deferred);
  const revoked = rollbackState(
    implemented,
    "approve",
    "d6da10b0-c980-4b82-b920-640238095f1a",
  );
  equal(revoked.approval, undefined);
  equal(revoked.planReview, state.planReview);
});

Deno.test("an approve_plan handoff also accepts an explicit deferral of remaining findings", () => {
  const state = planning();
  state.planReview!.findings = [
    { id: "naming", severity: "medium", description: "Unclear name" },
  ];
  const clean = StateSchema.parse(state);
  const current = handoff(clean, "approve_plan");
  const args = resolve(
    "approve",
    { handoffId, request: deferral, ignore: ["naming"] },
    clean,
    current,
  );
  equal(
    deferredPlanFindings(transition(clean, "approve", args))!.findings.map((
      f,
    ) => f.id),
    ["naming"],
  );
  // Nothing blocks, so --ignore-remaining alone has nothing to defer.
  rejects(
    () =>
      resolve(
        "approve",
        { handoffId, request: deferral, ignoreRemaining: true },
        clean,
        current,
      ),
    "no unresolved blocking plan findings",
  );
});
