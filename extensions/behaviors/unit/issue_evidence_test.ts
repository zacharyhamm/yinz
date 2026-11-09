import {
  createEvidenceMethods,
  type EvidenceContext,
} from "../issue_evidence.ts";
import {
  assignmentCore,
  AssignmentSchema,
  canonical,
  evidenceDigest,
  type RoleEvidence,
  SubmissionSchema,
  SubmitArgsSchema,
} from "../_lib/issue_evidence.ts";
import {
  codeReviewFingerprint,
  createState,
  transition,
} from "../_lib/lifecycle.ts";
import { assertSubmissionBackend } from "../_lib/runtime.ts";
import { AgentConfigSchema, promptDigest } from "../_lib/yinz_agent.ts";

function assert(value: unknown, message = "Assertion failed"): asserts value {
  if (!value) throw new Error(message);
}
function equal(a: unknown, b: unknown) {
  assert(canonical(a) === canonical(b), `${canonical(a)} != ${canonical(b)}`);
}
async function rejects(operation: () => unknown, text: string) {
  try {
    await operation();
  } catch (error) {
    assert(String(error).includes(text), String(error));
    return;
  }
  throw new Error(`Expected rejection containing ${text}`);
}
const issue = {
  workspace: "default",
  itemId: 85,
  text: "Fix issue",
  note: "Acceptance",
  digest: "d".repeat(64),
};
const taskId = "a6da10b0-c980-4b82-b920-000000000001";
const operationId = "a6da10b0-c980-4b82-b920-000000000002";
const head = "a".repeat(40);
const triage = { classification: "bug" as const, summary: "Reproduced" };
const plan = {
  summary: "Fix",
  steps: [{ id: "fix", description: "Fix source", files: ["src/lib.rs"] }],
  testingStrategy: "Focused",
  checks: [{
    id: "fix",
    command: ["cargo", "x", "test", "-p", "yinz-server"],
    covers: ["fix"],
  }],
};

async function fixture(role: RoleEvidence["role"] = "triage") {
  let state = createState(issue);
  if (role !== "triage") state = transition(state, "triage", triage);
  if (
    [
      "review_plan",
      "implement",
      "review_code",
      "review_ux",
      "plan_review_fixes",
      "implement_review_fixes",
    ].includes(role)
  ) {
    state = transition(state, "plan", plan);
  }
  if (
    [
      "implement",
      "review_code",
      "review_ux",
      "plan_review_fixes",
      "implement_review_fixes",
    ].includes(role)
  ) {
    state = transition(state, "review_plan", { planVersion: 1, findings: [] });
    state = transition(state, "approve", {
      planVersion: 1,
      request: "Implement",
    });
    state = transition(state, "implement", {
      worktree: "/repo/issue",
      branch: "issue-85",
    });
  }
  if (
    ["review_ux", "plan_review_fixes", "implement_review_fixes"].includes(role)
  ) {
    state = transition(state, "review_code", {
      planVersion: 1,
      commit: head,
      steps: [{ id: "fix", status: "implemented" }],
      findings: [],
    });
  }
  if (["plan_review_fixes", "implement_review_fixes"].includes(role)) {
    state = transition(state, "review_ux", {
      planVersion: 1,
      commit: head,
      findings: [{
        id: "ux",
        severity: "medium",
        description: "Feedback needs repair",
      }],
      assessment: {
        impact: "material",
        summary: "Feedback gap",
        assessedPaths: ["cli.ts:1"],
        limitations: [],
      },
    });
    state = transition(state, "decide_review", {
      id: taskId,
      reviewFingerprint: codeReviewFingerprint(state),
      request: "Plan UX repairs",
      repair: [{ role: "review_ux", findingId: "ux" }],
      ignore: [],
    });
    if (role === "implement_review_fixes") {
      state = transition(state, "plan_review_fixes", plan);
      state = transition(state, "approve_review_fixes", {
        id: taskId,
        handoffId: taskId,
        request: "Implement UX repairs",
      });
    }
  }
  const prompt = "Investigate once and submit through Swamp";
  const task = {
    id: taskId,
    behaviorName: `issue-agent-${taskId}`,
    role,
    before: state,
    config: AgentConfigSchema.parse({
      session: "dev",
      kind: "codex",
      cwd: "/repo/issue",
    }),
    status: "running",
    prompt,
    promptDigest: await promptDigest(prompt),
    ...([
        "review_code",
        "review_ux",
        "plan_review_fixes",
        "implement_review_fixes",
      ].includes(role)
      ? { reviewCommit: head }
      : {}),
  };
  const parent = {
    modelId: "parent-id",
    definition: { name: "issue-default-85" },
  };
  const core = assignmentCore(parent, task, "/repo/control");
  const assignment = AssignmentSchema.parse({
    ...core,
    digest: evidenceDigest(core),
    promptDigest: task.promptDigest,
  });
  const target = {
    protocol: 1,
    behaviorName: `issue-evidence-${taskId}`,
    controlDir: "/repo/control",
    assignmentDigest: assignment.digest,
  };
  const parents = new Map<string, unknown>([["state", state], ["agent-task", {
    ...task,
    submission: target,
  }]]);
  const records = new Map<string, unknown>();
  const writes: unknown[] = [];
  const controls = {
    failWrite: false,
    agenda: issue,
    clean: true,
    head,
    branch: "issue-85",
  };
  const context: EvidenceContext = {
    repoDir: "/repo/control",
    definition: { name: target.behaviorName },
    signal: new AbortController().signal,
    createFileWriter: () => {
      throw new Error("Submission cannot write command logs");
    },
    readResource: (name) =>
      Promise.resolve(structuredClone(records.get(name) ?? null)),
    writeResource: (_spec, name, value) => {
      if (controls.failWrite) {
        controls.failWrite = false;
        return Promise.reject(new Error("Write interrupted"));
      }
      records.set(name, structuredClone(value));
      writes.push(structuredClone(value));
      return Promise.resolve({ name });
    },
    dataRepository: {
      getContent: (type, name, resource) => {
        equal(type, "@yinz/issue-lifecycle");
        equal(name, parent.modelId);
        const value = parents.get(resource);
        return Promise.resolve(
          value == null
            ? null
            : new TextEncoder().encode(JSON.stringify(value)),
        );
      },
    },
  };
  const methods = createEvidenceMethods({
    realPath: (p) => Promise.resolve(p),
    now: () => "2026-11-08T18:00:00.000Z",
    readAgenda: () => Promise.resolve(controls.agenda),
    inspect: () =>
      Promise.resolve({
        commonDir: "/repo/.git",
        worktree: "/repo/issue",
        branch: controls.branch,
        head: controls.head,
        main: head,
        clean: controls.clean,
      }),
  });
  await methods.open.execute({ assignment }, context);
  const evidence: Record<RoleEvidence["role"], unknown> = {
    triage,
    plan,
    review_plan: { planVersion: 1, findings: [] },
    implement: { summary: "Implemented", commit: head },
    plan_review_fixes: plan,
    implement_review_fixes: { summary: "Repaired", commit: head },
    review_ux: {
      planVersion: 1,
      commit: head,
      findings: [],
      assessment: {
        impact: "no_material_impact",
        summary: "Caller use is unchanged",
        assessedPaths: ["src/lib.rs caller"],
        limitations: [],
      },
    },
    review_code: {
      planVersion: 1,
      commit: head,
      steps: [{ id: "fix", status: "implemented" }],
      findings: [],
    },
  };
  const input = (value: unknown = evidence[role]) =>
    SubmitArgsSchema.parse({
      taskId,
      assignmentDigest: assignment.digest,
      role,
      evidence: value,
    });
  const submit = (value?: unknown) =>
    methods.submit.execute(input(value), context);
  return {
    methods,
    context,
    assignment,
    task,
    target,
    parents,
    records,
    writes,
    controls,
    input,
    submit,
  };
}

for (
  const role of [
    "triage",
    "plan",
    "review_plan",
    "implement",
    "review_code",
    "review_ux",
    "plan_review_fixes",
    "implement_review_fixes",
  ] as const
) {
  Deno.test(`${role} evidence is validated and acknowledged in one Swamp resource`, async () => {
    const h = await fixture(role);
    await h.submit();
    const saved = SubmissionSchema.parse(h.records.get("submission"));
    assert(saved.status === "accepted");
    equal(saved.receipt.payload.role, role);
    equal(saved.receipt.taskId, taskId);
    equal(h.writes.length, 2);
    // No parent writes: submission authority cannot grant approval or merge.
    equal(
      (h.parents.get("agent-task") as { status: string }).status,
      "running",
    );
  });
}

Deno.test("invalid or truncated submission is rejected before acceptance and corrected with the same assignment", async () => {
  const h = await fixture("review_plan");
  await rejects(() => JSON.parse('{"findings":['), "JSON");
  await rejects(
    () =>
      h.submit({
        planVersion: 1,
        findings: [{
          id: "gap",
          severity: "high",
          description: "Missing evidence",
        }],
      }),
    "needs a requirement",
  );
  equal(SubmissionSchema.parse(h.records.get("submission")).status, "open");
  await h.submit();
  equal(SubmissionSchema.parse(h.records.get("submission")).status, "accepted");
});

Deno.test("duplicate submission preserves the original receipt without writes and changed evidence conflicts", async () => {
  const h = await fixture();
  await h.submit();
  const original = structuredClone(h.records.get("submission"));
  // Simulate losing the first command's acknowledgement, then replaying it.
  await h.submit({
    summary: triage.summary,
    classification: triage.classification,
  });
  equal(h.writes.length, 2);
  equal(h.records.get("submission"), original);
  await rejects(
    () => h.submit({ ...triage, summary: "Different" }),
    "Submission conflict",
  );
  equal(h.records.get("submission"), original);
});

Deno.test("failed persistence cannot acknowledge evidence and retries preserve the assignment", async () => {
  const h = await fixture();
  h.controls.failWrite = true;
  await rejects(h.submit, "Write interrupted");
  equal(SubmissionSchema.parse(h.records.get("submission")).status, "open");
  await h.submit();
  const saved = SubmissionSchema.parse(h.records.get("submission"));
  assert(saved.status === "accepted");
  equal(saved.assignment, h.assignment);
});

Deno.test("task role, prompt, requirements, generation, plan and rollback changes fence submissions", async () => {
  for (
    const change of [
      "task",
      "role",
      "prompt",
      "requirements",
      "generation",
      "plan",
      "rollback",
    ] as const
  ) {
    const h = await fixture();
    const parent = h.parents.get("agent-task") as Record<string, unknown>;
    const state = structuredClone(h.task.before);
    if (change === "task") parent.id = operationId;
    if (change === "role") parent.role = "plan";
    if (change === "prompt") parent.prompt = "Substituted instructions";
    if (change === "requirements") {
      h.controls.agenda = { ...issue, digest: "e".repeat(64) };
    }
    if (change === "generation") {
      state.planningContext = {
        generation: 1,
        initialPlanVersion: 0,
        automaticRevisionConsumed: false,
        userFeedback: [],
        history: [],
        adjudications: [],
        acceptedDecisions: [],
      };
    }
    if (change === "plan") state.planVersion++;
    if (change === "rollback") state.rollbackId = operationId;
    h.parents.set("state", state);
    await rejects(
      h.submit,
      change === "requirements" ? "requirements changed" : "stale",
    );
    equal(h.writes.length, 1);
  }
});

Deno.test("worker cannot submit another role, task, digest or controller mutation", async () => {
  const h = await fixture();
  await rejects(
    () =>
      h.methods.submit.execute(
        { ...h.input(), taskId: operationId },
        h.context,
      ),
    "different assignment",
  );
  await rejects(
    () =>
      h.methods.submit.execute({
        ...h.input(),
        assignmentDigest: "e".repeat(64),
      }, h.context),
    "different assignment",
  );
  await rejects(
    () =>
      h.methods.submit.execute(
        SubmitArgsSchema.parse({ ...h.input(), role: "plan", evidence: plan }),
        h.context,
      ),
    "role does not match",
  );
  await rejects(
    () => SubmitArgsSchema.parse({ ...h.input(), approve: true }),
    "Unrecognized key",
  );
  equal(h.writes.length, 1);
});

Deno.test("code evidence binds a clean branch and the dispatched reviewed commit", async () => {
  for (const role of ["review_code", "review_ux"] as const) {
    for (const change of ["dirty", "branch", "head", "reviewed"] as const) {
      const h = await fixture(role);
      if (change === "dirty") h.controls.clean = false;
      if (change === "branch") h.controls.branch = "other";
      if (change === "head") h.controls.head = "b".repeat(40);
      const evidence = {
        ...h.input().evidence,
        ...(change === "reviewed" ? { commit: "b".repeat(40) } : {}),
      };
      await rejects(
        () => h.submit(evidence),
        change === "reviewed" ? "different commit" : "clean issue branch",
      );
      equal(h.writes.length, 1);
    }
  }
});

Deno.test("controller supersession fences late submissions and retains earlier receipts", async () => {
  for (const accepted of [false, true]) {
    const h = await fixture();
    const args = { taskId, assignmentDigest: h.assignment.digest, operationId };
    await rejects(
      () => h.methods.supersede.execute(args, h.context),
      "durable controller",
    );
    if (accepted) await h.submit();
    const previous = SubmissionSchema.parse(h.records.get("submission"));
    h.parents.set("recovery", {
      id: operationId,
      status: "pending",
      task: { id: taskId },
    });
    await h.methods.supersede.execute(args, h.context);
    const saved = SubmissionSchema.parse(h.records.get("submission"));
    assert(saved.status === "superseded");
    if (previous.status === "accepted") equal(saved.receipt, previous.receipt);
    const writes = h.writes.length;
    await h.methods.supersede.execute(args, h.context);
    equal(h.writes.length, writes);
    await rejects(h.submit, "superseded");
  }
});

Deno.test("pending recovery and discard intents reject new evidence before parent state changes", async () => {
  for (const discard of [false, true]) {
    const h = await fixture();
    if (discard) {
      (h.parents.get("agent-task") as Record<string, unknown>).discard = {
        request: "Discard",
        confirmedStopped: true,
      };
    } else h.parents.set("recovery", { status: "pending" });
    await rejects(h.submit, "no longer accepting");
    equal(h.writes.length, 1);
  }
});

Deno.test("submission preflight rejects a shared writer lease before launching workers", async () => {
  assertSubmissionBackend({ type: "filesystem", healthy: true });
  assertSubmissionBackend({ type: "filesystem", healthy: true, format: 2 });
  for (
    const input of [{ type: "filesystem", healthy: true, format: 3 }, {
      type: "s3",
      healthy: true,
    }, { type: "filesystem", healthy: false }]
  ) {
    await rejects(
      () => assertSubmissionBackend(input),
      "No worker was launched",
    );
  }
});

Deno.test("UX evidence persistence replay conflicts and supersession retain original assignment", async () => {
  const h = await fixture("review_ux");
  h.controls.failWrite = true;
  await rejects(h.submit, "Write interrupted");
  equal(SubmissionSchema.parse(h.records.get("submission")).status, "open");
  await h.submit();
  const accepted = SubmissionSchema.parse(h.records.get("submission"));
  assert(accepted.status === "accepted");
  equal(accepted.assignment, h.assignment);
  await h.submit();
  equal(h.records.get("submission"), accepted);
  const payload = h.input().evidence as Extract<
    RoleEvidence,
    { role: "review_ux" }
  >["evidence"];
  await rejects(
    () =>
      h.submit({
        ...payload,
        assessment: { ...payload.assessment, summary: "Changed assessment" },
      }),
    "Submission conflict",
  );
  h.parents.set("recovery", {
    id: operationId,
    status: "pending",
    task: { id: taskId },
  });
  await h.methods.supersede.execute({
    taskId,
    assignmentDigest: h.assignment.digest,
    operationId,
  }, h.context);
  const old = SubmissionSchema.parse(h.records.get("submission"));
  assert(old.status === "superseded");
  equal(old.receipt, accepted.receipt);
  await rejects(h.submit, "superseded");
});

Deno.test("UX assignments reject changed requirements state plan role and pending recovery", async () => {
  for (
    const change of [
      "requirements",
      "plan",
      "rollback",
      "role",
      "task",
      "recovery",
      "discard",
    ] as const
  ) {
    const h = await fixture("review_ux");
    if (change === "requirements") {
      h.controls.agenda = { ...issue, digest: "e".repeat(64) };
    }
    if (change === "plan") {
      h.parents.set("state", { ...h.task.before, planVersion: 2 });
    }
    if (change === "rollback") {
      h.parents.set("state", { ...h.task.before, rollbackId: operationId });
    }
    const parent = h.parents.get("agent-task") as Record<string, unknown>;
    if (change === "role") parent.role = "review_code";
    if (change === "task") parent.id = operationId;
    if (change === "recovery") h.parents.set("recovery", { status: "pending" });
    if (change === "discard") {
      parent.discard = { request: "Discard", confirmedStopped: true };
    }
    await rejects(
      h.submit,
      ["recovery", "discard"].includes(change)
        ? "no longer accepting"
        : change === "requirements"
        ? "requirements changed"
        : "stale",
    );
    equal(SubmissionSchema.parse(h.records.get("submission")).status, "open");
  }
  const h = await fixture("review_ux");
  await rejects(
    () =>
      h.methods.submit.execute({
        ...h.input(),
        assignmentDigest: "e".repeat(64),
      }, h.context),
    "different assignment",
  );
  await rejects(
    () => h.submit({ ...h.input().evidence, planVersion: 2 }),
    "current plan version",
  );
});
