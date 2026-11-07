import {
  completeAgenda,
  createState,
  digestIssue,
  effectivePlanFindings,
  type Issue,
  type LifecycleState,
  LifecycleStepSchema,
  previousPlanFindings,
  readAgenda,
  refreshIssue,
  reopenAgenda,
  reviewFingerprint,
  rollbackState,
  transition,
  validatePlanReview,
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
};
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
  return transition(
    transition(implementing(), "review_code", review),
    "ready",
    { commit: head, mergeRequest: "Merge this issue" },
  );
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

Deno.test("merge authorization binds the reviewed commit and rejects failed checks", () => {
  const reviewed = transition(implementing(), "review_code", review);
  rejects(
    () =>
      transition(reviewed, "ready", {
        commit: changedHead,
        mergeRequest: "Merge",
      }),
    "reviewed commit",
  );
  const failed = transition(implementing(), "review_code", {
    ...review,
    preliminaryChecks: [{
      command: "cargo x test selected_test",
      result: "failed",
      summary: "Behavior regression",
    }],
  });
  rejects(
    () => transition(failed, "ready", { commit: head, mergeRequest: "Merge" }),
    "failed preliminary",
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
    () => transition(finding, "ready", { commit: head, mergeRequest: "Merge" }),
    "Resolve high/critical",
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
