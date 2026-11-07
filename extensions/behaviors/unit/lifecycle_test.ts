import {
  completeAgenda,
  createState,
  digestIssue,
  effectivePlanFindings,
  type Issue,
  type LifecycleState,
  LifecycleStepSchema,
  mergeGate,
  previousPlanFindings,
  readAgenda,
  recordMergeFailure,
  refreshIssue,
  reopenAgenda,
  reviewFingerprint,
  rollbackState,
  transition,
  validatePlanReview,
  VerificationSchema,
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
async function rejectsAsync(
  fn: () => Promise<unknown>,
  message: string,
): Promise<void> {
  try {
    await fn();
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
const changedHead = "b".repeat(40);
const issue: Issue = {
  workspace: "default",
  itemId: 85,
  text: "Fix issue",
  note: "Acceptance criteria",
  digest: "c".repeat(64),
};
const plan = {
  summary: "Fix the behavior and document it",
  steps: [
    { id: "fix", description: "Fix the implementation", files: ["src/lib.rs"] },
    { id: "docs", description: "Document the behavior", files: ["README.md"] },
  ],
  testingStrategy: "Run the affected crate tests",
  checks: [
    {
      id: "crate",
      command: ["cargo", "x", "test", "-p", "demo"],
      covers: ["fix"],
    },
    { id: "docs", command: ["test", "-s", "README.md"], covers: ["docs"] },
  ],
};
function verification(
  state: LifecycleState,
  outcomes: { exitCode: number | null; worktreeUnchanged?: boolean }[] = [
    { exitCode: 0 },
    { exitCode: 0 },
  ],
  commit = head,
) {
  const id = "b6da10b0-c980-4b82-b920-640238095f1a";
  const checks = state.plan!.checks!.map((check, index) => {
    const { exitCode, worktreeUnchanged = true } = outcomes[index];
    return {
      ...check,
      exitCode,
      durationMs: 1500,
      log: `check-log-${id}-${check.id}`,
      worktreeUnchanged,
      status: exitCode === 0 && worktreeUnchanged ? "passed" : "failed",
    };
  });
  return {
    verification: {
      id,
      planVersion: state.plan!.version,
      commit,
      branch: state.implementation!.branch,
      startedAt: "2026-11-03T11:00:00.000Z",
      finishedAt: "2026-11-03T11:05:00.000Z",
      status: checks.every((check) => check.status === "passed")
        ? "passed"
        : "failed",
      checks,
    },
  };
}
function reviewed(): LifecycleState {
  return transition(implementing(), "review_code", review);
}
function verified(): LifecycleState {
  const state = reviewed();
  return transition(state, "verify", verification(state));
}
const review = {
  planVersion: 1,
  commit: head,
  steps: [{ id: "fix", status: "implemented" }, {
    id: "docs",
    status: "implemented",
  }],
  findings: [],
};
function planned(): LifecycleState {
  let state = transition(createState(issue), "triage", {
    classification: "bug",
    summary: "Confirmed reproduction",
  });
  state = transition(state, "plan", plan);
  return state;
}
function implementing(): LifecycleState {
  let state = transition(planned(), "review_plan", {
    planVersion: 1,
    findings: [],
  });
  state = transition(state, "approve", {
    planVersion: 1,
    request: "Implement this plan",
  });
  return transition(state, "implement", {
    worktree: "/tmp/issue-85",
    branch: "issue-85",
  });
}
function ready(): LifecycleState {
  return transition(verified(), "ready", {
    commit: head,
    mergeRequest: "Merge this issue",
  });
}

function done(): LifecycleState {
  const attempt = {
    id: "a6da10b0-c980-4b82-b920-640238095f1a",
    originalHead: head,
    startedAt: "2026-11-03T12:00:00.000Z",
  };
  const state = transition(ready(), "begin_merge", { attempt });
  return transition(
    transition(state, "merged", {
      receipt: {
        attemptId: attempt.id,
        originalCommit: head,
        rebasedCommit: changedHead,
        baseCommit: "d".repeat(40),
        mergedCommit: changedHead,
        worktree: state.implementation!.worktree,
        branch: state.implementation!.branch,
        ciStatus: "passed",
        mergedAt: "2026-11-03T12:10:00.000Z",
      },
    }),
    "done",
    { outcomeMet: true, summary: "Delivered" },
  );
}

Deno.test("every lifecycle step can be restarted while retaining only prerequisite evidence", () => {
  const original = done();
  const snapshot = structuredClone(original);
  const id = "a6da10b0-c980-4b82-b920-640238095f1b";
  const fields = [
    "classification",
    "plan",
    "planReview",
    "approval",
    "implementation",
    "codeReview",
    "mergeAuthorization",
    "receipt",
  ] as const;
  for (const [index, step] of LifecycleStepSchema.options.entries()) {
    const state = rollbackState(original, step, id);
    equal(state.planVersion, original.planVersion);
    equal(state.rollbackId, id);
    equal(state.implementation?.worktree, original.implementation?.worktree);
    equal(state.implementation?.branch, original.implementation?.branch);
    assert(!state.outcome && !state.attempt && !state.lastFailure);
    for (const [fieldIndex, field] of fields.entries()) {
      // Worktree ownership survives rollback; already-landed merges are retained for retries.
      if (field === "implementation" || (field === "receipt" && index >= 7)) {
        continue;
      }
      equal(Boolean(state[field]), fieldIndex < index);
    }
    if (index <= 5) assert(!state.implementation?.head);
    if (index >= 7) equal(state.receipt, original.receipt);
  }
  equal(original, snapshot);
  // Plan numbers never rewind: evidence for an abandoned version cannot authorize a new plan.
  const replanned = transition(
    rollbackState(original, "plan", id),
    "plan",
    plan,
  );
  equal(replanned.plan?.version, 2);
  rejects(
    () =>
      transition(replanned, "approve", {
        planVersion: 1,
        request: "Implement",
      }),
    "current plan",
  );
});

Deno.test("rollback cannot skip missing prerequisites and permits retrying the initial step", () => {
  const initial = createState(issue);
  const id = "a6da10b0-c980-4b82-b920-640238095f1b";
  equal(rollbackState(initial, "triage", id).phase, "triaging");
  for (const step of LifecycleStepSchema.options.slice(1)) {
    rejects(() => rollbackState(initial, step, id), "missing triage evidence");
  }
  equal(rollbackState(ready(), "merge", id).phase, "ready");
  rejects(
    () => rollbackState(ready(), "summarize", id),
    "missing merge evidence",
  );
});

Deno.test("rollback reopens a completed Agenda task idempotently and verifies its description", async () => {
  let completed = true;
  let reopenCount = 0;
  const expected = { ...issue, digest: await digestIssue(issue) };
  const command = (args: string[]) => {
    if (args.includes("reopen")) {
      equal(args.at(-1), String(issue.itemId));
      completed = false;
      reopenCount++;
      return Promise.resolve({});
    }
    return Promise.resolve({
      schema_version: 0,
      item: {
        ...agendaItem(),
        text: issue.text,
        note: issue.note,
        completed,
      },
    });
  };
  await reopenAgenda(expected, command);
  await reopenAgenda(expected, command);
  equal(reopenCount, 1);
  await rejectsAsync(
    () => reopenAgenda({ ...expected, digest: "f".repeat(64) }, command),
    "description changed",
  );
  equal(reopenCount, 1);
});

Deno.test("plan revision invalidates approval, code review and merge authorization", () => {
  const previous = ready();
  const state = transition(previous, "plan", {
    ...plan,
    summary: "A revised approach",
  });
  equal(state.phase, "planning");
  equal(state.plan?.version, 2);
  assert(
    !state.approval && !state.planReview && !state.codeReview &&
      !state.mergeAuthorization,
  );
  assert(!state.implementation?.head);
  equal(previous.phase, "ready");
  rejects(
    () =>
      transition(state, "approve", { planVersion: 1, request: "Implement it" }),
    "current plan",
  );
  rejects(
    () =>
      transition(state, "approve", { planVersion: 2, request: "Implement it" }),
    "needs a review",
  );
});

Deno.test("approval requires review and resolution of high or critical findings", () => {
  const finding = {
    id: "race",
    severity: "high",
    description: "Concurrent merge can overwrite evidence",
  };
  let state = transition(planned(), "review_plan", {
    planVersion: 1,
    findings: [finding],
  });
  rejects(
    () =>
      transition(state, "approve", { planVersion: 1, request: "Implement" }),
    "Resolve high/critical",
  );
  state = transition(state, "review_plan", {
    planVersion: 1,
    findings: [{
      ...finding,
      resolution: "Serialize merges with a shared lock",
    }],
  });
  // Rolling back approval retains the current review and its supported dispositions.
  equal(
    effectivePlanFindings(rollbackState(state, "approve", crypto.randomUUID())),
    effectivePlanFindings(state),
  );
  equal(
    transition(state, "approve", { planVersion: 1, request: "Implement" })
      .phase,
    "approved",
  );
});

Deno.test("code review covers every plan step once and explains deviations", () => {
  const state = implementing();
  rejects(
    () =>
      transition(state, "review_code", {
        ...review,
        steps: review.steps.slice(0, 1),
      }),
    "every plan step",
  );
  rejects(
    () =>
      transition(state, "review_code", {
        ...review,
        steps: [review.steps[0], review.steps[0]],
      }),
    "every plan step",
  );
  rejects(
    () =>
      transition(state, "review_code", {
        ...review,
        steps: [{ id: "fix", status: "implemented" }, {
          id: "docs",
          status: "deviated",
        }],
      }),
    "justification",
  );
  const accepted = transition(state, "review_code", {
    ...review,
    steps: [{ id: "fix", status: "implemented" }, {
      id: "docs",
      status: "deviated",
      justification: "Existing documentation already specifies this behavior",
    }],
  });
  equal(accepted.codeReview?.steps[1].status, "deviated");
});

function legacy(state: LifecycleState): LifecycleState {
  const copy = structuredClone(state);
  delete copy.plan!.checks;
  return copy;
}

Deno.test("merge authorization binds the reviewed commit and rejects findings", () => {
  rejects(
    () =>
      transition(verified(), "ready", {
        commit: changedHead,
        mergeRequest: "Merge",
      }),
    "reviewed commit",
  );
  const finding = transition(implementing(), "review_code", {
    ...review,
    findings: [{
      id: "broken",
      severity: "critical",
      description: "Data loss",
    }],
  });
  rejects(
    () =>
      transition(
        transition(finding, "verify", verification(finding)),
        "ready",
        { commit: head, mergeRequest: "Merge" },
      ),
    "Resolve high/critical",
  );
});

Deno.test("new plans require structured checks that cover existing steps", () => {
  const triaged = transition(createState(issue), "triage", {
    classification: "bug",
    summary: "Confirmed reproduction",
  });
  const { checks: _checks, ...withoutChecks } = plan;
  rejects(() => transition(triaged, "plan", withoutChecks), "checks");
  rejects(
    () => transition(triaged, "plan", { ...plan, checks: [] }),
    "checks",
  );
  rejects(
    () =>
      transition(triaged, "plan", {
        ...plan,
        checks: [plan.checks[0], { ...plan.checks[1], id: "crate" }],
      }),
    "Check IDs must be unique",
  );
  rejects(
    () =>
      transition(triaged, "plan", {
        ...plan,
        checks: [{ ...plan.checks[0], covers: ["fix", "missing"] }],
      }),
    "covers unknown plan steps: missing",
  );
  for (const command of [[], [""], ["cargo", "a\0b"]]) {
    rejects(
      () =>
        transition(triaged, "plan", {
          ...plan,
          checks: [{ ...plan.checks[0], command }],
        }),
      "command",
    );
  }
  rejects(
    () =>
      transition(triaged, "plan", {
        ...plan,
        checks: [{ ...plan.checks[0], id: "Bad ID" }],
      }),
    "Check IDs",
  );
});

Deno.test("verification is bound to the reviewed commit, branch, plan and exact checks", () => {
  const state = reviewed();
  rejects(
    () =>
      transition(state, "verify", verification(state, undefined, changedHead)),
    "reviewed commit",
  );
  const branch = verification(state);
  branch.verification.branch = "other-branch";
  rejects(() => transition(state, "verify", branch), "issue branch");
  const version = verification(state);
  version.verification.planVersion = 2;
  rejects(() => transition(state, "verify", version), "current plan version");
  const reordered = verification(state);
  reordered.verification.checks.reverse();
  rejects(() => transition(state, "verify", reordered), "exactly as approved");
  const edited = verification(state);
  edited.verification.checks[0].command = ["/usr/bin/true"];
  rejects(() => transition(state, "verify", edited), "exactly as approved");
  const dropped = verification(state);
  dropped.verification.checks.pop();
  rejects(() => transition(state, "verify", dropped), "exactly as approved");
  const lying = verification(state, [{ exitCode: 1 }, { exitCode: 0 }]);
  lying.verification.status = "passed";
  rejects(() => transition(state, "verify", lying), "exit codes");
  const dirty = verification(state, [
    { exitCode: 0, worktreeUnchanged: false },
    { exitCode: 0 },
  ]);
  dirty.verification.checks[0].status = "passed";
  rejects(() => transition(state, "verify", dirty), "exit codes");
  rejects(
    () => transition(implementing(), "verify", verification(state)),
    "code review",
  );
  rejects(
    () => transition(legacy(state), "verify", verification(state)),
    "legacy plan",
  );
  // An empty list would be neither legacy nor verifiable.
  const empty = structuredClone(state);
  empty.plan!.checks = [];
  rejects(() => transition(empty, "verify", verification(state)), "checks");
});

Deno.test("ready requires a passing verification of the reviewed commit; reviewer checks are advisory", () => {
  const state = reviewed();
  rejects(
    () => transition(state, "ready", { commit: head, mergeRequest: "Merge" }),
    "Run verify",
  );
  const failed = transition(
    state,
    "verify",
    verification(state, [{ exitCode: 0 }, { exitCode: 2 }]),
  );
  equal(failed.verification?.status, "failed");
  rejects(
    () => transition(failed, "ready", { commit: head, mergeRequest: "Merge" }),
    "Verification failed for docs",
  );
  // An explicit rerun at the same commit replaces the failed run.
  const rerun = transition(failed, "verify", verification(failed));
  equal(rerun.verification?.status, "passed");
  equal(
    transition(rerun, "ready", { commit: head, mergeRequest: "Merge" }).phase,
    "ready",
  );
  const advisory = transition(implementing(), "review_code", {
    ...review,
    preliminaryChecks: [{
      command: "cargo x test selected_test",
      result: "failed",
      summary: "Reviewer saw a failure",
    }],
  });
  equal(
    transition(
      transition(advisory, "verify", verification(advisory)),
      "ready",
      { commit: head, mergeRequest: "Merge" },
    ).phase,
    "ready",
  );
});

Deno.test("mergeGate is the single ready/verified rule", () => {
  const state = reviewed();
  const gate = (value: LifecycleState) =>
    mergeGate(value.plan!, value.codeReview!, value.verification);
  equal(gate(state), { kind: "unverified" });
  const passed = verified();
  equal(gate(passed).kind, "passed");
  const failed = transition(
    state,
    "verify",
    verification(state, [{ exitCode: 0 }, { exitCode: 2 }]),
  );
  equal(gate(failed).kind, "failed");
  // A verification of another commit or plan version is not current evidence.
  for (
    const stale of [
      { ...passed.verification!, commit: changedHead },
      { ...passed.verification!, planVersion: 2 },
    ]
  ) {
    equal(
      mergeGate(passed.plan!, passed.codeReview!, stale),
      { kind: "unverified" },
    );
  }
  equal(gate(legacy(passed)), { kind: "legacy", failedPreliminary: false });
  equal(
    mergeGate(legacy(passed).plan!, {
      ...passed.codeReview!,
      preliminaryChecks: [{
        command: "cargo x test selected_test",
        result: "failed",
        summary: "Behavior regression",
      }],
    }, undefined),
    { kind: "legacy", failedPreliminary: true },
  );
});

Deno.test("a timed-out check result has no exit code and cannot pass", () => {
  const { verification: record } = verification(reviewed(), [
    { exitCode: null },
    { exitCode: 0 },
  ]);
  const timedOut = structuredClone(record);
  Object.assign(timedOut.checks[0], { timedOut: true });
  assert(VerificationSchema.safeParse(timedOut).success);
  const withExit = structuredClone(timedOut);
  Object.assign(withExit.checks[0], { exitCode: 0 });
  assert(!VerificationSchema.safeParse(withExit).success);
});

Deno.test("legacy plans without checks keep the reviewer-reported gate", () => {
  const state = legacy(reviewed());
  equal(
    transition(state, "ready", { commit: head, mergeRequest: "Merge" }).phase,
    "ready",
  );
  const failed = legacy(transition(implementing(), "review_code", {
    ...review,
    preliminaryChecks: [{
      command: "cargo x test selected_test",
      result: "failed",
      summary: "Behavior regression",
    }],
  }));
  rejects(
    () => transition(failed, "ready", { commit: head, mergeRequest: "Merge" }),
    "failed preliminary",
  );
});

Deno.test("verification goes stale after repair, a new review, a new plan, refresh or rollback", () => {
  const state = verified();
  const stale = [
    transition(state, "repair", { reason: "Fix a flaky check" }),
    transition(state, "review_code", { ...review, commit: changedHead }),
    transition(state, "plan", plan),
    transition(state, "implement", {
      worktree: "/tmp/issue-85",
      branch: "issue-85",
    }),
    refreshIssue(state, { ...issue, note: "Changed", digest: "d".repeat(64) }),
    rollbackState(state, "review_code", "a6da10b0-c980-4b82-b920-640238095f1c"),
    rollbackState(state, "implement", "a6da10b0-c980-4b82-b920-640238095f1d"),
  ];
  for (const next of stale) {
    assert(!next.verification, JSON.stringify(next));
    rejects(
      () => transition(next, "ready", { commit: head, mergeRequest: "Merge" }),
      "",
    );
  }
  // Rolling back only the merge decision keeps verification of the same commit.
  const authorized = transition(state, "ready", {
    commit: head,
    mergeRequest: "Merge",
  });
  equal(
    rollbackState(authorized, "ready", "a6da10b0-c980-4b82-b920-640238095f1e")
      .verification,
    state.verification,
  );
});

Deno.test("Agenda description change invalidates decisions but preserves plan numbering and worktree", () => {
  const state = refreshIssue(ready(), {
    ...issue,
    note: "Changed acceptance criteria",
    digest: "d".repeat(64),
  });
  equal(state.phase, "triaging");
  equal(state.implementation?.worktree, "/tmp/issue-85");
  assert(
    !state.plan && !state.classification && !state.approval &&
      !state.codeReview && !state.mergeAuthorization,
  );
  const next = transition(
    transition(state, "triage", {
      classification: "bug",
      summary: "Updated reproduction",
    }),
    "plan",
    plan,
  );
  equal(next.plan?.version, 2);
  rejects(
    () => refreshIssue(state, { ...issue, itemId: 86 }),
    "different Agenda identity",
  );
});

Deno.test("merge receipt requires matching attempt, reviewed commit and explicit CI skip request", () => {
  const attempt = {
    id: "a6da10b0-c980-4b82-b920-640238095f1a",
    originalHead: head,
    startedAt: "2026-11-03T12:00:00Z",
  };
  const state = transition(ready(), "begin_merge", { attempt });
  rejects(
    () =>
      transition(ready(), "begin_merge", {
        attempt: { ...attempt, id: "not-a-uuid" },
      }),
    "UUID",
  );
  rejects(
    () =>
      transition(implementing(), "review_code", {
        ...review,
        commit: "a".repeat(41),
      }),
    "Invalid string",
  );
  const receipt = {
    attemptId: attempt.id,
    originalCommit: head,
    rebasedCommit: changedHead,
    baseCommit: "e".repeat(40),
    mergedCommit: changedHead,
    worktree: "/tmp/issue-85",
    branch: "issue-85",
    ciStatus: "passed",
    mergedAt: "2026-11-03T13:00:00Z",
  };
  rejects(
    () => transition(ready(), "merged", { receipt }),
    "active authorized attempt",
  );
  rejects(
    () =>
      transition(state, "merged", {
        receipt: { ...receipt, originalCommit: changedHead },
      }),
    "active authorized attempt",
  );
  rejects(
    () =>
      transition(state, "merged", {
        receipt: { ...receipt, ciStatus: "skipped" },
      }),
    "explicit request",
  );
  rejects(
    () =>
      transition(state, "merged", {
        receipt: { ...receipt, mergedCommit: head },
      }),
    "verified rebased commit",
  );
  rejects(
    () =>
      transition(state, "merged", {
        receipt: { ...receipt, skipCiRequest: "Skip CI" },
      }),
    "cannot also claim",
  );
  const skipCiRequest = "Skip CI for this merge";
  const skipped = transition(ready(), "begin_merge", {
    attempt: { ...attempt, skipCiRequest },
  });
  equal(
    transition(skipped, "merged", {
      receipt: { ...receipt, ciStatus: "skipped", skipCiRequest },
    }).phase,
    "merged",
  );
  rejects(() => transition(state, "plan", plan), "active merge attempt");
  const merged = transition(state, "merged", { receipt });
  equal(merged.phase, "merged");
  rejects(
    () =>
      transition(merged, "done", {
        summary: "Still broken",
        outcomeMet: false,
      }),
    "true",
  );
  equal(
    transition(merged, "done", {
      summary: "Acceptance criteria verified",
      outcomeMet: true,
    }).phase,
    "done",
  );
  rejects(
    () => transition(merged, "repair", { reason: "Undo it" }),
    "requires",
  );
  rejects(
    () => refreshIssue(merged, { ...issue, digest: "f".repeat(64) }),
    "requires",
  );
});

Deno.test("repair retains approved plan but requires fresh code review and merge authorization", () => {
  const state = transition(ready(), "begin_merge", {
    attempt: {
      id: "a6da10b0-c980-4b82-b920-640238095f1a",
      originalHead: head,
      skipCiRequest: "Skip CI for this merge",
      startedAt: "2026-11-03T12:00:00Z",
    },
  });
  const repaired = transition(state, "repair", {
    reason: "CI failed after rebase",
  });
  equal(repaired.phase, "implementing");
  assert(repaired.plan && repaired.approval);
  assert(
    !repaired.attempt && !repaired.codeReview && !repaired.mergeAuthorization,
  );
  rejects(
    () =>
      transition(repaired, "ready", { commit: head, mergeRequest: "Merge" }),
    "reviewed commit",
  );
});

Deno.test("a failed merge attempt is recorded once and survives repair, a later merge and rollback", () => {
  const attempt = {
    id: "a6da10b0-c980-4b82-b920-640238095f1a",
    originalHead: head,
    startedAt: "2026-11-03T12:00:00Z",
  };
  const started = transition(ready(), "begin_merge", { attempt });
  const failure = {
    attemptId: attempt.id,
    at: "2026-11-03T12:30:00.000Z",
    originalCommit: head,
    summary: "phases failed: test; tests: demo tests::flaky; exit 101",
    failure: "cargo x ci check failed: phases failed: test",
    ciStatus: "failed",
    log: "/repo/.git/yinz-lifecycle/merges/a6da/ci-check.log",
    logResource: `merge-log-${attempt.id}`,
    exitCode: 101,
    phases: [{ name: "test", status: "failed" as const }],
    failedTests: ["demo tests::flaky"],
  };
  rejects(
    () => recordMergeFailure(ready(), failure),
    "active attempt",
  );
  rejects(
    () =>
      recordMergeFailure(started, { ...failure, originalCommit: changedHead }),
    "active attempt",
  );
  const failed = recordMergeFailure(started, failure);
  equal(failed.mergeFailures, [failure]);
  equal(
    failed.lastFailure,
    `Merge attempt ${attempt.id} failed: ${failure.summary}; log: ${failure.log}`,
  );
  // A retry of the failed attempt keeps the original evidence.
  const retried = recordMergeFailure(
    { ...failed, lastFailure: "This attempt failed; create a new attempt" },
    { ...failure, summary: "This attempt failed" },
  );
  equal(retried.mergeFailures, [failure]);
  rejects(() => transition(failed, "plan", plan), "active merge attempt");
  const repaired = transition(failed, "repair", { reason: "Fix the flake" });
  equal(repaired.mergeFailures, [failure]);
  equal(
    repaired.lastFailure,
    `Fix the flake\n\nMerge attempt ${attempt.id} failed: ${failure.summary}; log: ${failure.log}`,
  );
  assert(!repaired.attempt);
  const second = transition(ready(), "begin_merge", {
    attempt: { ...attempt, id: "b6da10b0-c980-4b82-b920-640238095f1a" },
  });
  const merged = transition(
    { ...second, mergeFailures: repaired.mergeFailures },
    "merged",
    {
      receipt: {
        attemptId: "b6da10b0-c980-4b82-b920-640238095f1a",
        originalCommit: head,
        rebasedCommit: changedHead,
        baseCommit: "e".repeat(40),
        mergedCommit: changedHead,
        worktree: "/tmp/issue-85",
        branch: "issue-85",
        ciStatus: "passed",
        mergedAt: "2026-11-03T13:00:00Z",
        previousAttempts: [{
          attemptId: attempt.id,
          summary: failure.summary,
          log: failure.log,
        }],
      },
    },
  );
  equal(merged.mergeFailures, [failure]);
  equal(merged.receipt?.previousAttempts?.[0].attemptId, attempt.id);
  equal(rollbackState(merged, "implement", crypto.randomUUID()).mergeFailures, [
    failure,
  ]);
});

Deno.test("issue digest includes exact description and identity in deterministic key order", async () => {
  const digest = await digestIssue(issue);
  equal(
    digest,
    await digestIssue({
      note: issue.note,
      text: issue.text,
      itemId: issue.itemId,
      workspace: issue.workspace,
    }),
  );
  assert(digest !== await digestIssue({ ...issue, note: issue.note + "\n" }));
  assert(digest !== await digestIssue({ ...issue, workspace: "other" }));
  assert(digest !== await digestIssue({ ...issue, itemId: 86 }));
});

function agendaItem() {
  return {
    id: 85,
    kind: "task",
    text: "Fix issue",
    note: "Acceptance criteria",
    completed: false,
    blocked: false,
    discarded_at: null,
    assignments: [],
  };
}

Deno.test("Agenda reads reject unsupported schemas and ineligible issue records", async () => {
  const cases: [Record<string, unknown>, string][] = [
    [{ kind: "note" }, "reference notes"],
    [{ discarded_at: 1 }, "trashed"],
    [{ completed: true }, "completed"],
    [{ blocked: true }, "prerequisites"],
    [{ id: 86 }, "different item ID"],
    [{
      assignments: [{
        category: 1,
        item: 85,
        source: "Manual",
        value: {
          date: {
            temporal: { kind: "floating", date: "2026-11-03" },
            recurrence: { frequency: "daily" },
          },
        },
      }],
    }, "Recurring"],
  ];
  for (const [patch, message] of cases) {
    await rejectsAsync(
      () =>
        readAgenda(
          "default",
          85,
          false,
          () =>
            Promise.resolve({
              schema_version: 0,
              item: { ...agendaItem(), ...patch },
            }),
        ),
      message,
    );
  }
  await rejectsAsync(
    () =>
      readAgenda(
        "default",
        85,
        false,
        () => Promise.resolve({ schema_version: 1, item: agendaItem() }),
      ),
    "0",
  );
  const observed: string[][] = [];
  const value = await readAgenda("default", 85, false, (args) => {
    observed.push(args);
    return Promise.resolve({ schema_version: 0, item: agendaItem() });
  });
  equal(observed, [["--workspace", "default", "--json", "item", "show", "85"]]);
  equal(value.digest, await digestIssue(value));
});

Deno.test("Agenda completion is idempotent, checks the description, and never rewrites it", async () => {
  const captured = { ...issue, digest: await digestIssue(issue) };
  let completed = false;
  const calls: string[][] = [];
  const command = (args: string[]): Promise<unknown> => {
    calls.push(args);
    const item = { ...agendaItem(), completed };
    if (args[4] === "done") {
      completed = true;
      return Promise.resolve({
        schema_version: 0,
        items: [{ ...item, completed }],
        completion: {
          completed: item.completed ? 0 : 1,
          reopened: 0,
          created: 0,
          created_items: [],
        },
      });
    }
    return Promise.resolve({ schema_version: 0, item });
  };
  equal(await completeAgenda(captured, command), captured);
  equal(await completeAgenda(captured, command), captured);
  assert(calls.every((args) => ["show", "done"].includes(args[4])));
  let writes = 0;
  await rejectsAsync(() =>
    completeAgenda(captured, (args) => {
      if (args[4] === "done") writes += 1;
      return Promise.resolve({
        schema_version: 0,
        item: { ...agendaItem(), note: "New requirements" },
      });
    }), "description changed");
  equal(writes, 0);
});

Deno.test("adjudications preserve the review, gate by effective severity and reject stale or invalid dispositions", () => {
  const findings = ["resolved", "rejected", "downgraded"].map((id) => ({
    id,
    severity: "high",
    description: id,
    requirement: "Correct caller behavior",
    mechanism: "Caller bypasses validation",
    evidence: ["src/lib.rs:42"],
  }));
  const original = transition(planned(), "review_plan", {
    planVersion: 1,
    findings,
  });
  const input = {
    planVersion: 1,
    reviewFingerprint: reviewFingerprint(original),
    dispositions: findings.map((f) => ({
      findingId: f.id,
      disposition: f.id,
      rationale: "Inspected the guarded caller",
      evidence: ["src/caller.rs:20 checks the input"],
      ...(f.id === "downgraded" ? { severity: "medium" } : {}),
    })),
  };
  const state = transition(original, "adjudicate_plan", input);
  equal(state.planReview, original.planReview);
  equal(
    effectivePlanFindings(state).map((f) =>
      !!f.resolution || f.severity === "medium"
    ),
    [true, true, true],
  );
  equal(
    transition(state, "approve", { planVersion: 1, request: "Implement" })
      .phase,
    "approved",
  );
  rejects(
    () =>
      transition(original, "approve", { planVersion: 1, request: "Implement" }),
    "Resolve high/critical",
  );
  for (
    const stale of [
      transition(original, "plan", plan),
      transition(original, "review_plan", { planVersion: 1, findings: [] }),
      rollbackState(original, "review_plan", crypto.randomUUID()),
    ]
  ) {
    rejects(() => transition(stale, "adjudicate_plan", input), "current");
    assert(effectivePlanFindings(stale).every((f) => !f.resolution));
  }
  rejects(() =>
    transition(original, "adjudicate_plan", {
      ...input,
      dispositions: [{ ...input.dispositions[0], findingId: "unknown" }],
    }), "Unknown finding");
  rejects(() =>
    transition(original, "adjudicate_plan", {
      ...input,
      dispositions: [{ ...input.dispositions[2], severity: "critical" }],
    }), "lower");
  rejects(() =>
    transition(original, "adjudicate_plan", {
      ...input,
      dispositions: [{ ...input.dispositions[0], evidence: [] }],
    }), "Too small");
  rejects(() =>
    transition(original, "adjudicate_plan", {
      ...input,
      dispositions: [{ ...input.dispositions[0], disposition: "accepted" }],
    }), "Invalid option");
});

Deno.test("reopening an adjudicated or fixed concern requires new evidence addressing its disposition", () => {
  const finding = {
    id: "caller",
    severity: "high" as const,
    description: "Caller gap",
    requirement: "Validate input",
    mechanism: "Caller bypasses the guard",
    evidence: ["src/lib.rs:42"],
  };
  let state = transition(planned(), "review_plan", {
    planVersion: 1,
    findings: [finding],
  });
  state = transition(state, "adjudicate_plan", {
    planVersion: 1,
    reviewFingerprint: reviewFingerprint(state),
    dispositions: [{
      findingId: "caller",
      disposition: "rejected",
      rationale: "Guard exists",
      evidence: ["src/caller.rs:20"],
    }],
  });
  const review = { planVersion: 1, findings: [finding] };
  rejects(() => validatePlanReview(state, review), "Reopening caller");
  state = transition(state, "plan", plan);
  equal(
    state.planningContext?.history.findLast((h) => h.adjudications.length)
      ?.adjudications[0].dispositions[0].disposition,
    "rejected",
  );
  rejects(
    () => validatePlanReview(state, { ...review, planVersion: 2 }),
    "Reopening caller",
  );
  rejects(
    () =>
      validatePlanReview(state, {
        planVersion: 2,
        findings: [{
          ...finding,
          reopening: { rationale: "Still broken", evidence: ["src/lib.rs:42"] },
        }],
      }),
    "new evidence",
  );
  validatePlanReview(state, {
    planVersion: 2,
    findings: [{
      ...finding,
      reopening: {
        rationale: "The guard excludes the remote caller",
        evidence: ["src/remote.rs:81"],
      },
    }],
  });
  const fixed = transition(
    transition(transition(planned(), "review_plan", review), "plan", plan),
    "review_plan",
    { planVersion: 2, findings: [] },
  );
  rejects(
    () => validatePlanReview(fixed, { ...review, planVersion: 2 }),
    "Reopening caller",
  );
});

Deno.test("refresh alone renews the allowance and history never becomes current review evidence", () => {
  let state = transition(planned(), "review_plan", {
    planVersion: 1,
    findings: [],
  });
  state = transition(state, "plan", plan);
  assert(state.planningContext?.automaticRevisionConsumed);
  state = rollbackState(state, "plan", crypto.randomUUID());
  assert(state.planningContext?.automaticRevisionConsumed);
  const unchanged = refreshIssue(state, issue);
  assert(unchanged.planningContext?.automaticRevisionConsumed);
  state = refreshIssue(state, {
    ...issue,
    note: "New requirement",
    digest: "e".repeat(64),
  });
  equal(state.planningContext?.generation, 1);
  equal(state.planningContext?.automaticRevisionConsumed, false);
  equal(state.planReview, undefined);
  assert(state.planningContext!.history.length > 0);
  state = transition(state, "triage", {
    classification: "feature",
    summary: "Investigated",
  });
  state = transition(state, "plan", plan);
  equal(state.planVersion, 3);
  equal(state.planningContext?.automaticRevisionConsumed, false);
  rejects(
    () => transition(state, "approve", { planVersion: 3, request: "Approve" }),
    "needs a review",
  );
});

Deno.test("a reopened concern can persist, and downgraded findings retain their disposition across a planning rollback", () => {
  const finding = {
    id: "gap",
    severity: "high" as const,
    description: "Caller gap",
    requirement: "Validate caller",
    mechanism: "Caller can skip validation",
    evidence: ["src/caller.rs:20"],
  };
  let state = transition(planned(), "review_plan", {
    planVersion: 1,
    findings: [finding],
  });
  const input = {
    planVersion: 1,
    reviewFingerprint: reviewFingerprint(state),
    dispositions: [{
      findingId: "gap",
      disposition: "downgraded",
      severity: "medium",
      rationale: "Bounded impact",
      evidence: ["src/guard.rs:10"],
    }],
  };
  state = transition(state, "adjudicate_plan", input);
  equal(transition(state, "adjudicate_plan", input), state);
  validatePlanReview(state, {
    planVersion: 1,
    findings: [{ ...finding, severity: "medium" }],
  });
  rejects(
    () => validatePlanReview(state, { planVersion: 1, findings: [finding] }),
    "new evidence",
  );
  const rolled = rollbackState(state, "plan", crypto.randomUUID());
  equal(previousPlanFindings(rolled)[0].severity, "medium");
  state = transition(state, "plan", plan);
  const reopened = {
    planVersion: 2,
    findings: [{
      ...finding,
      reopening: {
        rationale: "Remote callers affect all sessions",
        evidence: ["src/remote.rs:81"],
      },
    }],
  };
  validatePlanReview(state, reopened);
  state = transition(state, "review_plan", reopened);
  state = transition(state, "plan", plan);
  validatePlanReview(state, { planVersion: 3, findings: [finding] });
});

Deno.test("open findings may escalate, dispositions accumulate, and history keeps one entry per review", () => {
  const finding = {
    id: "gap",
    severity: "medium" as const,
    description: "Caller gap",
  };
  let state = transition(planned(), "review_plan", {
    planVersion: 1,
    findings: [finding],
  });
  validatePlanReview(state, {
    planVersion: 1,
    findings: [{
      ...finding,
      severity: "high",
      requirement: "Validate caller",
      mechanism: "Caller can skip validation",
      evidence: ["src/caller.rs:20"],
    }],
  });
  const fingerprint = reviewFingerprint(state);
  const disposition = {
    findingId: "gap",
    rationale: "Guard exists",
    evidence: ["src/guard.rs:10"],
  };
  state = transition(state, "adjudicate_plan", {
    planVersion: 1,
    reviewFingerprint: fingerprint,
    dispositions: [{ ...disposition, disposition: "rejected" }],
  });
  state = transition(state, "adjudicate_plan", {
    planVersion: 1,
    reviewFingerprint: fingerprint,
    dispositions: [{
      ...disposition,
      disposition: "downgraded",
      severity: "low",
    }],
  });
  equal(effectivePlanFindings(state)[0].resolution, "Guard exists");
  equal(effectivePlanFindings(state)[0].severity, "low");
  state = transition(state, "plan", plan);
  state = transition(state, "review_plan", { planVersion: 2, findings: [] });
  state = transition(state, "plan", plan);
  equal(
    state.planningContext!.history.map((h) => [h.plan.version, !!h.review]),
    [[1, true], [2, true]],
  );
});
