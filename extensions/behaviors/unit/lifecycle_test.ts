import {
  completeAgenda,
  createState,
  digestIssue,
  type Issue,
  type LifecycleState,
  readAgenda,
  refreshIssue,
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
