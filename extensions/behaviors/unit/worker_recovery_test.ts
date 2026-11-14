import { AgentTaskSchema } from "../_lib/issue_agents.ts";
import { createState } from "../_lib/lifecycle.ts";
import {
  classifyWorkerFailure,
  correlatedTimeout,
  RECOVERY_DEADLINE_MS,
  RECOVERY_POLICY,
  RecoveryBudget,
  type RetryClass,
} from "../_lib/worker_recovery.ts";
import { type AgentState, AgentStateSchema } from "../_lib/yinz_agent.ts";

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

const timestamp = "2026-11-03T12:00:00.000Z";
const issue = {
  workspace: "default",
  itemId: 85,
  text: "Fix issue",
  note: "Acceptance criteria",
  digest: "d".repeat(64),
};
const config = {
  session: "development",
  control: { session: "development", paneId: "terminal_1" },
  stackWorkers: true,
  kind: "claude" as const,
  cwd: "/repo/control",
  startupTimeoutSeconds: 120,
  timeoutSeconds: 600,
};
type Task = ReturnType<typeof AgentTaskSchema.parse>;

function task(patch: Record<string, unknown> = {}): Task {
  return AgentTaskSchema.parse({
    id: "a6da10b0-c980-4b82-b920-000000000001",
    behaviorName: "issue-agent-a6da10b0-c980-4b82-b920-000000000001",
    role: "triage",
    config,
    prompt: "Investigate",
    promptDigest: "0".repeat(64),
    before: createState(issue),
    createdAt: timestamp,
    status: "running",
    submission: {
      protocol: 1,
      behaviorName: "issue-evidence-a6da10b0-c980-4b82-b920-000000000001",
      controlDir: "/repo/control",
      assignmentDigest: "1".repeat(64),
    },
    ...patch,
  });
}

function agentRow(patch: Record<string, unknown> = {}) {
  return {
    pane_id: "terminal_7",
    session: config.session,
    agent: { kind: "claude", session_id: "conversation-1", state: "Busy" },
    cwd: config.cwd,
    ready: false,
    status_available: true,
    prompt_supported: true,
    not_ready_reason: "not_idle",
    ...patch,
  };
}

function child(
  patch: Record<string, unknown> = {},
  response: Record<string, unknown> = {},
  row: Record<string, unknown> = {},
): AgentState {
  return AgentStateSchema.parse({
    config,
    prompt: "Investigate",
    promptDigest: "0".repeat(64),
    phase: "waiting",
    launchVersion: 2,
    launchServer: "server-1",
    paneId: "terminal_7",
    conversationId: "conversation-1",
    submissionId: "submission-1",
    outcome: "timed_out",
    failureKind: "timeout",
    lastError: "Agent task has not completed: timed_out",
    rawResponse: JSON.stringify({
      outcome: "timed_out",
      submission_id: "submission-1",
      acknowledged: true,
      prompt_delivery: "started",
      server_instance: "server-1",
      agents: [agentRow(row)],
      ...response,
    }),
    backgroundTasks: [],
    createdAt: timestamp,
    updatedAt: timestamp,
    ...patch,
  });
}

/** e4dcd266: background idle wait timed out while input was unconsumed. */
function backgroundChild(patch: Record<string, unknown> = {}) {
  return child(
    {
      phase: "background",
      turnId: "turn-1",
      backgroundTasks: ["worker"],
      lastError: "Background work has not completed: timed_out",
      ...patch,
    },
    {
      submission_id: undefined,
      acknowledged: false,
      prompt_delivery: undefined,
    },
    { not_ready_reason: "unconsumed_input" },
  );
}

/** e6644a0e: a resumed submission wait while the phase stayed `submitting`. */
function submittingChild(response: Record<string, unknown> = {}) {
  return child({ phase: "submitting" }, response);
}

function errorFor(state: AgentState, prefixed = false) {
  return new Error(`${prefixed ? "Error: " : ""}${state.lastError}`);
}

function verdict(
  state: AgentState | undefined,
  options: {
    task?: Task;
    receiptAccepted?: boolean;
    error?: unknown;
  } = {},
) {
  return classifyWorkerFailure({
    task: options.task ?? task(),
    child: state,
    receiptAccepted: options.receiptAccepted ?? false,
    error: options.error ??
      (state ? errorFor(state) : new Error("no child")),
  });
}

Deno.test("correlated timeouts accept unconsumed_input, acknowledged submitting replies and background idle waits", () => {
  const accepted: [string, AgentState][] = [
    ["waiting not_idle", child()],
    [
      "waiting unconsumed_input",
      child({}, {}, { not_ready_reason: "unconsumed_input" }),
    ],
    [
      "waiting unresolved_turn",
      child({}, {}, {
        not_ready_reason: "unresolved_turn",
      }),
    ],
    ["submitting acknowledged", submittingChild()],
    ["background unconsumed_input", backgroundChild()],
  ];
  for (const [label, state] of accepted) {
    assert(correlatedTimeout(task(), state, errorFor(state)), label);
    assert(correlatedTimeout(task(), state, errorFor(state, true)), label);
    equal(verdict(state).kind, "retry");
  }
  const rejected: [string, AgentState][] = [
    [
      "Waiting dialog",
      child({}, {}, {
        agent: {
          kind: "claude",
          session_id: "conversation-1",
          state: "Waiting",
        },
      }),
    ],
    [
      "unknown not_ready_reason",
      child({}, {}, { not_ready_reason: "clearing_draft" }),
    ],
    [
      "submitting without acknowledgement",
      submittingChild({
        acknowledged: false,
      }),
    ],
    [
      "submitting without started delivery",
      submittingChild({
        prompt_delivery: undefined,
      }),
    ],
    [
      "submitting other submission",
      submittingChild({
        submission_id: "submission-2",
      }),
    ],
    ["not_sent delivery", child({}, { prompt_delivery: "not_sent" })],
    ["mismatched server", child({}, { server_instance: "server-2" })],
    [
      "mismatched turn",
      child({ turnId: "turn-1" }, { turn_id: "turn-2" }),
    ],
    ["mismatched cwd", child({}, {}, { cwd: "/replacement" })],
    [
      "mismatched conversation",
      child({}, {}, {
        agent: {
          kind: "claude",
          session_id: "conversation-2",
          state: "Busy",
        },
      }),
    ],
    ["other failure kind", child({ failureKind: "other" })],
    ["completed phase", child({ phase: "completed" })],
  ];
  for (const [label, state] of rejected) {
    assert(!correlatedTimeout(task(), state, errorFor(state)), label);
  }
  // A stale lastError belongs to an older failure.
  const stale = child();
  assert(
    !correlatedTimeout(task(), stale, new Error("Session disappeared")),
    "stale lastError",
  );
  assert(!correlatedTimeout(task(), stale, "not an error"), "non-error");
});

Deno.test("classifier keeps human-required outcomes blocked", () => {
  const legacy = task({ config: { ...config, control: undefined } });
  const continuation = task({
    continuation: {
      id: "a6da10b0-c980-4b82-b920-000000000009",
      request: "Continue",
      prompt: "Finish",
    },
  });
  const rows: [
    string,
    AgentState | undefined,
    {
      task?: Task;
      receiptAccepted?: boolean;
      error?: unknown;
    }?,
  ][] = [
    [
      "needs_input",
      child({ outcome: "needs_input", failureKind: "other" }, {
        outcome: "needs_input",
      }),
    ],
    [
      "target_changed",
      child({ outcome: "target_changed", failureKind: "other" }),
    ],
    [
      "target_closed",
      child({ outcome: "target_closed", failureKind: "other" }),
    ],
    [
      "failed KickedByHost",
      child({
        outcome: "failed",
        failureKind: "other",
        lastError: "Agent task has not completed: failed: KickedByHost",
      }, { outcome: "failed" }),
    ],
    [
      "status_unavailable",
      child({ outcome: "status_unavailable", failureKind: "other" }),
    ],
    [
      "unacknowledged interrupted",
      child({
        outcome: "interrupted",
        failureKind: "other",
        lastError:
          "Agent task has not completed: interrupted: Submission was interrupted before acknowledgement; acceptance is uncertain",
      }, { outcome: "interrupted", acknowledged: false }),
    ],
    [
      "submission-wait interrupted",
      child({
        outcome: "interrupted",
        failureKind: "other",
        observedVia: "submission",
        lastError: "Agent task has not completed: interrupted",
      }, { outcome: "interrupted" }),
    ],
    [
      "launch_rejected",
      child({
        phase: "launch_rejected",
        outcome: "rejected",
        failureKind: "other",
        launchId: "server-1.launch",
        paneId: undefined,
        conversationId: undefined,
        submissionId: undefined,
      }),
    ],
    [
      "uncertain launch",
      child({
        phase: "launching",
        outcome: "transport_error",
        failureKind: "other",
        launchId: "server-1.launch",
        paneId: undefined,
        conversationId: undefined,
        submissionId: undefined,
      }),
    ],
    [
      "hooks_unavailable preflight",
      child({
        phase: "preflight",
        outcome: "hooks_unavailable",
        failureKind: "other",
        paneId: undefined,
        conversationId: undefined,
        submissionId: undefined,
      }),
    ],
    [
      "legacy preflight without control",
      child({
        config: legacy.config,
        phase: "preflight",
        outcome: "preflight_transport",
        failureKind: "other",
        paneId: undefined,
        conversationId: undefined,
        submissionId: undefined,
      }),
      { task: legacy },
    ],
    ["missing child", undefined, {
      error: new Error("Pane task has no durable agent state"),
    }],
    [
      "completed child without accepted receipt",
      child({ phase: "completed", outcome: "completed" }),
      {
        receiptAccepted: false,
        error: new Error("submission_required: provider turn ended"),
      },
    ],
    [
      "unrecorded continuation",
      child({ outcome: "completed", phase: "completed" }),
      { task: continuation, receiptAccepted: true },
    ],
    [
      "runner exception after timeout",
      child(),
      { error: new Error("Error: Session disappeared") },
    ],
    [
      "discard intent",
      child(),
      {
        task: task({
          discard: { request: "Discard", confirmedStopped: true },
        }),
      },
    ],
    ["superseded task", child(), { task: task({ status: "superseded" }) }],
  ];
  for (const [label, state, options] of rows) {
    const result = verdict(state, options);
    equal(result.kind, "human");
    assert(result.reason.length > 0, label);
  }
  assert(
    verdict(child({ outcome: "needs_input", failureKind: "other" }))
      .reason.includes("dialog"),
  );
});

Deno.test("completion_unreconciled requires an accepted receipt, completed child and no background work", () => {
  const completed = child({ phase: "completed", outcome: "completed" });
  const error = new Error("Swamp child response lost");
  equal(verdict(completed, { receiptAccepted: true, error }), {
    kind: "retry",
    class: "reobserve",
    reason:
      "completion_unreconciled: the child completed and its evidence is accepted",
  });
  equal(verdict(completed, { receiptAccepted: false, error }).kind, "human");
  equal(
    verdict(
      child({
        phase: "completed",
        outcome: "completed",
        backgroundTasks: ["worker"],
      }),
      { receiptAccepted: true, error },
    ).kind,
    "human",
  );
  equal(
    verdict(child({ phase: "waiting", outcome: "completed" }), {
      receiptAccepted: true,
      error,
    }).kind,
    "human",
  );
  // A legacy task without a typed submission has no receipt to reconcile.
  equal(
    verdict(completed, {
      task: task({ submission: undefined }),
      receiptAccepted: true,
      error,
    }).kind,
    "human",
  );
  equal(
    verdict(completed, {
      task: task({ status: "applying" }),
      receiptAccepted: true,
      error,
    }).kind,
    "retry",
  );
});

Deno.test("recorded transport errors after submission are re-observed only when fresh", () => {
  const transport = child({
    outcome: "transport_error",
    failureKind: "other",
    lastError: "Yinz returned no valid agent response (exit 1): lost",
  });
  equal(verdict(transport), {
    kind: "retry",
    class: "reobserve",
    reason: "recorded transport error after submission",
  });
  equal(
    verdict(transport, { error: new Error("Session disappeared") }).kind,
    "human",
  );
  // The schema trims lastError; Swamp keeps the CLI output's trailing newline.
  const raw = "Yinz returned no valid agent response (exit 1): lost\n";
  const trimmed = child({
    outcome: "transport_error",
    failureKind: "other",
    lastError: raw,
  });
  equal(trimmed.lastError, raw.trim());
  for (const message of [raw, `Error: ${raw}`]) {
    equal(verdict(trimmed, { error: new Error(message) }).kind, "retry");
  }
  equal(
    verdict(trimmed, {
      error: new Error(
        "Yinz returned no valid agent response (exit 1): gone\n",
      ),
    }).kind,
    "human",
  );
  equal(
    verdict(child({
      outcome: "transport_error",
      failureKind: "other",
      phase: "spawned",
      submissionId: undefined,
    })).kind,
    "human",
  );
});

Deno.test("cleanup retry requires a fresh retryable refusal for the same submission and conversation", () => {
  const completed = task({ status: "completed", cleanup: "pending" });
  const thrown = "Worker cleanup blocked: rejected: worker is Busy, not idle";
  const refusal = {
    outcome: "rejected",
    message: "worker is Busy, not idle",
    thrown,
    retryable: true,
    submissionId: "submission-1",
    conversationId: "conversation-1",
    observedAt: timestamp,
  };
  const cleaned = (patch: Record<string, unknown> = {}) =>
    child({
      phase: "completed",
      outcome: "completed",
      cleanupRefusal: { ...refusal, ...patch },
    });
  const retry = (state: AgentState, error = new Error(thrown)) =>
    verdict(state, { task: completed, receiptAccepted: true, error });
  equal(retry(cleaned()).kind, "retry");
  equal(retry(cleaned(), new Error(`Error: ${thrown}`)).kind, "retry");
  // cleanupTask wraps texts that do not already name worker cleanup.
  for (
    const text of [
      "Cannot inspect worker before cleanup",
      "Worker cleanup returned no valid agent response (exit 1): ",
    ]
  ) {
    const wrapped = text.startsWith("Worker cleanup")
      ? text
      : `Worker cleanup blocked: ${text}`;
    const state = cleaned({
      outcome: "list_panes_failed",
      message: undefined,
      thrown: text,
    });
    equal(retry(state, new Error(wrapped)).kind, "retry");
    // Swamp may prefix the child's text before cleanupTask wraps it.
    equal(
      retry(state, new Error(`Worker cleanup blocked: Error: ${text}`)).kind,
      "retry",
    );
  }
  const stale = retry(
    cleaned(),
    new Error(
      "Worker cleanup blocked: rejected: worker background work is still running",
    ),
  );
  equal(stale.kind, "human");
  assert(stale.reason.startsWith("cleanup refusal requires inspection"));
  equal(retry(cleaned({ submissionId: "submission-2" })).kind, "human");
  equal(retry(cleaned({ conversationId: "conversation-2" })).kind, "human");
  equal(retry(cleaned({ retryable: false })).kind, "human");
  equal(
    retry(child({ phase: "completed", outcome: "completed" })).kind,
    "human",
  );
  // Only a completed task with pending cleanup is a cleanup failure.
  equal(
    verdict(cleaned(), {
      task: task({ status: "completed", cleanup: "done" }),
      error: new Error(thrown),
    }).kind,
    "human",
  );
  equal(
    verdict(cleaned(), { task: task(), error: new Error(thrown) }).kind,
    "human",
  );
});

Deno.test("preflight retry requires preflight_transport with no launch intent, pane or submission", () => {
  const preflight = (patch: Record<string, unknown> = {}) =>
    child({
      phase: "preflight",
      outcome: "preflight_transport",
      failureKind: "other",
      lastError:
        "Launch preflight transport failure (list-panes): Cannot resolve control pane: This session is updating Yinz; retry the action after reconnecting.; launch was not sent",
      paneId: undefined,
      conversationId: undefined,
      submissionId: undefined,
      launchServer: undefined,
      rawResponse: undefined,
      ...patch,
    });
  equal(verdict(preflight()), {
    kind: "retry",
    class: "preflight",
    reason: "launch preflight transport failure before any launch intent",
  });
  equal(verdict(preflight({ launchId: "server-1.launch" })).kind, "human");
  equal(verdict(preflight({ paneId: "terminal_7" })).kind, "human");
  equal(verdict(preflight({ submissionId: "submission-1" })).kind, "human");
  equal(verdict(preflight({ outcome: "hooks_unavailable" })).kind, "human");
  equal(verdict(preflight({ phase: "launching" })).kind, "human");
  equal(
    verdict(preflight(), { error: new Error("Unexpected end of JSON input") })
      .kind,
    "human",
  );
  // A trailing newline in the reported message is still the same failure.
  const message = `${preflight().lastError}\n`;
  for (const error of [new Error(message), new Error(`Error: ${message}`)]) {
    equal(verdict(preflight({ lastError: message }), { error }).kind, "retry");
  }
});

Deno.test("long_wait freshness ignores whitespace the child schema trims", () => {
  const raw = "Agent task has not completed: timed_out\n";
  const state = child({ lastError: raw });
  for (const error of [new Error(raw), new Error(`Error: ${raw}`)]) {
    const result = verdict(state, { error });
    equal(result.kind === "retry" && result.class, "long_wait");
  }
  // A different failure is not made fresh by trimming.
  equal(
    verdict(state, { error: new Error("Agent task has not completed: x\n") })
      .kind,
    "human",
  );
});

Deno.test("RecoveryBudget bounds attempts per task and class, resets for a new task id, and enforces the absolute deadline including the pending delay", () => {
  for (
    const [kind, delays] of [
      ["long_wait", Array(48).fill(0)],
      ["reobserve", [1_000, 5_000]],
      ["cleanup", [2_000, 5_000, 10_000, 20_000, 30_000, 60_000]],
      ["preflight", [5_000, 15_000, 30_000]],
    ] as [RetryClass, number[]][]
  ) {
    let clock = 0;
    const budget = new RecoveryBudget(() => clock);
    const granted: number[] = [];
    for (let i = 0; i < delays.length; i++) {
      const grant = budget.take("task-1", kind);
      assert(grant.allowed, `${kind} ${i}`);
      equal(grant.attempt, i + 1);
      equal(grant.limit, RECOVERY_POLICY[kind].maxAttempts);
      granted.push(grant.delayMs);
      clock += grant.delayMs;
    }
    equal(granted, delays);
    const refused = budget.take("task-1", kind);
    equal(refused.allowed, false);
    equal(refused.exhausted, "attempts");
    equal(refused.attempt, delays.length);
    // Another class and another task have their own allowance.
    assert(budget.take("task-2", kind).allowed);
    assert(
      budget.take("task-1", kind === "cleanup" ? "reobserve" : "cleanup")
        .allowed,
    );
  }
  let clock = 0;
  const budget = new RecoveryBudget(() => clock);
  clock = RECOVERY_DEADLINE_MS - 1_000;
  // The pending 2 s delay would cross the deadline.
  const late = budget.take("task-1", "cleanup");
  equal(late.allowed, false);
  equal(late.exhausted, "deadline");
  equal(late.attempt, 0);
  // A zero delay still fits before the deadline.
  assert(budget.take("task-1", "long_wait").allowed);
  clock = RECOVERY_DEADLINE_MS + 1;
  equal(budget.take("task-1", "long_wait").exhausted, "deadline");
  equal(budget.take("task-2", "long_wait").exhausted, "deadline");
});

Deno.test("drive classification matches status advice in the shared recovery fixture", async () => {
  const cases = JSON.parse(
    await Deno.readTextFile(
      new URL(
        "../../../scripts/issue_lifecycle/unit/fixtures/recovery_cases.json",
        import.meta.url,
      ),
    ),
  ) as {
    name: string;
    child: unknown;
    receiptAccepted: boolean;
    expected: { drive: string | null; status: string };
  }[];
  assert(cases.length >= 15, "the shared recovery fixture lost cases");
  for (const { name, child, receiptAccepted, expected } of cases) {
    const state = AgentStateSchema.parse(child);
    // task_recovery says drive only where drive retries, or observes before
    // any failure is recorded (no error for drive to classify yet).
    assert(
      (expected.status === "resume") === (expected.drive !== "human"),
      `${name}: status ${expected.status} disagrees with drive ${expected.drive}`,
    );
    if (expected.drive === null) {
      assert(!state.lastError, `${name}: a recorded failure needs a verdict`);
      continue;
    }
    const result = verdict(state, {
      task: task({ config: state.config, promptDigest: state.promptDigest }),
      receiptAccepted,
      error: state.lastError
        ? errorFor(state)
        : new Error("Swamp child response lost"),
    });
    equal(
      [name, result.kind === "retry" ? `retry:${result.class}` : "human"],
      [name, expected.drive],
    );
  }
});
