import {
  AgentTaskSchema,
  assertNoPendingAgent,
  createAgentMethods,
  type IssueAgentContext,
  type IssueAgentDependencies,
} from "../_lib/issue_agents.ts";
import {
  createState,
  type LifecycleState,
  refreshIssue,
  transition,
} from "../_lib/lifecycle.ts";
import { loadState, saveState } from "../_lib/runtime.ts";
import {
  AgentConfigSchema,
  type AgentContext,
  AgentStateSchema,
  confirmFailureAgent,
  promptDigest,
  recoverAgent,
  runAgent,
  type YinzRunner,
} from "../_lib/yinz_agent.ts";

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
async function rejects(fn: () => unknown, message: string): Promise<void> {
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

const issue = {
  workspace: "default",
  itemId: 85,
  text: "Fix issue",
  note: "Acceptance criteria",
  digest: "d".repeat(64),
};
const head = "a".repeat(40);
const triage = {
  classification: "bug",
  summary: "Reproduced against the source",
};
const options = {
  session: "development",
  kind: "claude" as const,
  startupTimeoutSeconds: 120,
  timeoutSeconds: 600,
};
const timestamp = "2026-11-03T12:00:00.000Z";

function implementing(): LifecycleState {
  let state = transition(createState(issue), "triage", triage);
  state = transition(state, "plan", {
    summary: "Repair issue",
    steps: [{
      id: "fix",
      description: "Repair and test",
      files: ["src/lib.rs"],
    }],
    testingStrategy: "Affected tests",
  });
  state = transition(state, "review_plan", { planVersion: 1, findings: [] });
  state = transition(state, "approve", {
    planVersion: 1,
    request: "Implement this plan",
  });
  return transition(state, "implement", {
    worktree: "/repo/issue",
    branch: "issue-85",
  });
}

function harness(initial = createState(issue), evidence: unknown = triage) {
  const resources = new Map<string, unknown>([["state", initial]]);
  const child = new Map<string, Record<string, unknown>>();
  const writes: {
    spec: string;
    name: string;
    value: Record<string, unknown>;
  }[] = [];
  const calls: Parameters<IssueAgentContext["runModel"]>[0][] = [];
  let serial = 0;
  const controls = {
    evidence,
    agenda: issue,
    failEventOnce: false,
    failTaskCompletionOnce: false,
    childPhase: "completed",
    childOutcome: "completed",
    childBackground: [] as string[],
    childPatch: {} as Record<string, unknown>,
    resultPatch: {} as Record<string, unknown>,
    runError: undefined as string | undefined,
    agentRunner: undefined as YinzRunner | undefined,
    agendaReads: 0,
    afterRun: () => {},
    repo: {
      commonDir: "/repo/.git",
      worktree: "/repo/issue",
      branch: "issue-85",
      head,
      main: "b".repeat(40),
      clean: true,
    },
  };
  const context: IssueAgentContext = {
    repoDir: "/repo/control",
    modelId: "lifecycle-model",
    definition: { name: "issue-default-85" },
    globalArgs: { workspace: "default", itemId: 85 },
    signal: new AbortController().signal,
    logger: { info() {} },
    readResource: (name) =>
      Promise.resolve(structuredClone(resources.get(name) ?? null)),
    writeResource: (spec, name, value) => {
      if (spec === "event" && controls.failEventOnce) {
        controls.failEventOnce = false;
        return Promise.reject(new Error("Fixture event write failed"));
      }
      if (
        spec === "agent-task" && value.status === "completed" &&
        controls.failTaskCompletionOnce
      ) {
        controls.failTaskCompletionOnce = false;
        return Promise.reject(
          new Error("Fixture task completion write failed"),
        );
      }
      writes.push({ spec, name, value: structuredClone(value) });
      resources.set(name, structuredClone(value));
      return Promise.resolve({ name });
    },
    createFileWriter: () => {
      throw new Error("Unexpected command log");
    },
    dataRepository: {
      getContent: (type, name, resource) => {
        equal(type, "@yinz/agent");
        equal(
          name,
          AgentTaskSchema.parse(resources.get("agent-task")).behaviorName,
        );
        const value = child.get(resource);
        return Promise.resolve(
          value ? new TextEncoder().encode(JSON.stringify(value)) : null,
        );
      },
    },
    runModel: async (request) => {
      calls.push(request);
      if (controls.agentRunner) {
        const task = AgentTaskSchema.parse(resources.get("agent-task"));
        const agentContext: AgentContext = {
          globalArgs: task.config,
          readResource: (name) =>
            Promise.resolve(structuredClone(child.get(name) ?? null)),
          writeResource: (_spec, name, value) => {
            child.set(name, structuredClone(value));
            return Promise.resolve({ name });
          },
        };
        const dependencies = {
          run: controls.agentRunner,
          realPath: (path: string) => Promise.resolve(path),
          id: () => "submission-1",
          now: () => timestamp,
        };
        try {
          if (request.method === "run") {
            child.clear();
            await runAgent({ prompt: task.prompt }, agentContext, dependencies);
          } else if (request.method === "confirm_failure") {
            await confirmFailureAgent(agentContext, dependencies);
          } else {
            equal(request.method, "recover");
            await recoverAgent(
              { prompt: task.prompt },
              agentContext,
              dependencies,
            );
          }
          return { ok: true };
        } catch (error) {
          return { ok: false, error: { message: String(error) } };
        }
      }
      if (request.method === "run") {
        const task = AgentTaskSchema.parse(resources.get("agent-task"));
        equal(request.name, task.behaviorName);
        const { prompt, ...rawConfig } = request.arguments!;
        assert(typeof prompt === "string");
        const config = AgentConfigSchema.parse(rawConfig);
        const digest = await promptDigest(prompt);
        const message = typeof controls.evidence === "string"
          ? controls.evidence
          : JSON.stringify(controls.evidence);
        child.set("state", {
          config,
          prompt,
          promptDigest: digest,
          phase: controls.childPhase,
          paneId: "terminal_7",
          conversationId: "conversation-1",
          submissionId: "submission-1",
          outcome: controls.childOutcome,
          message,
          turnId: "turn-1",
          backgroundTasks: controls.childBackground,
          createdAt: timestamp,
          updatedAt: timestamp,
          ...controls.childPatch,
        });
        child.set("result", {
          session: config.session,
          kind: config.kind,
          cwd: config.cwd,
          paneId: "terminal_7",
          conversationId: "conversation-1",
          submissionId: "submission-1",
          promptDigest: digest,
          outcome: "completed",
          message,
          turnId: "turn-1",
          completedAt: timestamp,
          ...controls.resultPatch,
        });
      }
      controls.afterRun();
      return controls.runError
        ? { ok: false, error: { message: controls.runError } }
        : { ok: true };
    },
  };
  const dependencies: IssueAgentDependencies = {
    withIssue: (_context, operation) => operation(),
    readAgenda: () => {
      controls.agendaReads++;
      return Promise.resolve(controls.agenda);
    },
    inspect: () => Promise.resolve(controls.repo),
    realPath: (path) => Promise.resolve(path),
    id: () => `a6da10b0-c980-4b82-b920-${String(++serial).padStart(12, "0")}`,
    now: () => timestamp,
  };
  return {
    context,
    controls,
    resources,
    writes,
    calls,
    child,
    methods: createAgentMethods(dependencies),
  };
}

Deno.test("issue agent dispatch persists prompt and configuration then imports correlated triage evidence", async () => {
  const h = harness();
  const input = h.methods.run_agent.arguments.parse({
    ...options,
    role: "triage",
    workspace: "default",
    itemId: 85,
  });
  await h.methods.run_agent.execute(input, h.context);
  equal(h.calls.length, 1);
  equal(h.calls[0].modelType, "@yinz/agent");
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(task.config.cwd, "/repo/control");
  equal(task.promptDigest, await promptDigest(task.prompt));
  assert(
    task.prompt.includes(
      "do not edit source files, Git refs, or configuration",
    ),
  );
  assert(task.prompt.includes("Do not invoke the lifecycle launcher"));
  equal(task.status, "completed");
  equal((h.resources.get("state") as LifecycleState).phase, "classified");
  await assertNoPendingAgent(h.context);
  await h.methods.resume_agent.execute({}, h.context);
  equal(h.calls.length, 1);
});

Deno.test("issue agent accepts the child's schema-normalized final message", async () => {
  const h = harness();
  h.controls.childPatch = { message: `\n${JSON.stringify(triage)}\n` };
  await h.methods.run_agent.execute({ ...options, role: "triage" }, h.context);
  equal((h.resources.get("state") as LifecycleState).phase, "classified");
});

Deno.test("issue agent rejects result identity mismatches and incomplete child state", async () => {
  const cases = [
    { resultPatch: { session: "another-session" } },
    { resultPatch: { submissionId: "another-submission" } },
    { resultPatch: { paneId: "terminal_8" } },
    { resultPatch: { promptDigest: "e".repeat(64) } },
    {
      resultPatch: {
        message: JSON.stringify({ ...triage, summary: "Substituted response" }),
      },
    },
    { childPatch: { prompt: "A different assignment" } },
    { childPhase: "waiting" },
    { childBackground: ["worker-still-running"] },
  ];
  for (const controls of cases) {
    const h = harness();
    Object.assign(h.controls, controls);
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
      "Pane",
    );
    equal((h.resources.get("state") as LifecycleState).phase, "triaging");
    await rejects(() => assertNoPendingAgent(h.context), "unresolved");
  }
});

Deno.test("malformed completed evidence can be explicitly discarded without applying or resubmitting it", async () => {
  const h = harness(createState(issue), "This is not the requested JSON");
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "required JSON evidence",
  );
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "unresolved",
  );
  await h.methods.discard_agent.execute({
    request: "Discard this malformed response; the pane has finished",
    confirmedStopped: true,
  }, h.context);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "discarded",
  );
  equal((h.resources.get("state") as LifecycleState).phase, "triaging");
  await assertNoPendingAgent(h.context);
  await h.methods.resume_agent.execute({}, h.context);
  equal(h.calls.length, 1);
  h.controls.evidence = triage;
  await h.methods.run_agent.execute({ ...options, role: "triage" }, h.context);
  equal((h.resources.get("state") as LifecycleState).phase, "classified");
  assert(h.calls[0].name !== h.calls[1].name);
});

Deno.test("missing final text can be explicitly discarded before dispatching replacement work", async () => {
  for (
    const { message, background } of [null, undefined, "", " \n\t"].flatMap((
      message,
    ) => [false, true].map((background) => ({ message, background })))
  ) {
    const h = harness();
    let cliCalls = 0;
    h.controls.agentRunner = (args) => {
      cliCalls++;
      const value = args[3] === "spawn"
        ? {
          outcome: "ok",
          spawned: {
            pane_id: "terminal_7",
            cwd: args[args.indexOf("--cwd") + 1],
          },
        }
        : {
          outcome: args[3] === "prompt" ? "completed" : "ok",
          agents: [{
            pane_id: "terminal_7",
            session: options.session,
            agent: {
              kind: "claude",
              session_id: "conversation-1",
              state: "idle",
            },
            ready: true,
            status_available: true,
            prompt_supported: true,
          }],
          submission_id: args[3] === "prompt" ? "submission-1" : undefined,
          message: background && args[3] === "prompt" ? "Working" : message,
          background_tasks: background && args[3] === "prompt"
            ? ["worker"]
            : [],
        };
      return Promise.resolve({
        code: 0,
        stdout: JSON.stringify(value),
        stderr: "",
      });
    };
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
      "discard-agent",
    );
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(
      AgentStateSchema.parse(h.child.get("state")).phase,
      "completed_unusable",
    );
    assert(!h.child.has("result"));
    await rejects(
      () => h.methods.resume_agent.execute({}, h.context),
      "discard-agent",
    );
    equal(cliCalls, background ? 4 : 3);
    await rejects(() => assertNoPendingAgent(h.context), "unresolved");
    assert(
      !h.methods.discard_agent.arguments.safeParse({
        request: "Discard empty answer",
        confirmedStopped: false,
      }).success,
    );
    const discard = {
      request: "Discard empty answer",
      confirmedStopped: true as const,
    };
    h.resources.set("state", transition(createState(issue), "triage", triage));
    await rejects(
      () => h.methods.discard_agent.execute(discard, h.context),
      "may already be applied",
    );
    h.resources.set("state", task.before);
    await h.methods.discard_agent.execute(discard, h.context);
    equal(h.resources.get("state"), task.before);
    equal(
      AgentTaskSchema.parse(h.resources.get("agent-task")).status,
      "discarded",
    );
    await assertNoPendingAgent(h.context);
    h.controls.agentRunner = undefined;
    await h.methods.run_agent.execute(
      { ...options, role: "triage" },
      h.context,
    );
    equal((h.resources.get("state") as LifecycleState).phase, "classified");
    assert(
      AgentTaskSchema.parse(h.resources.get("agent-task")).behaviorName !==
        task.behaviorName,
    );
  }
});

Deno.test("discard cannot unblock a live or uncertain pane assignment", async () => {
  const h = harness();
  h.controls.childPhase = "waiting";
  h.controls.childOutcome = "timed_out";
  h.controls.runError = "Provider timed out";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Provider timed out",
  );
  await rejects(
    () =>
      h.methods.discard_agent.execute({
        request: "Discard",
        confirmedStopped: true,
      }, h.context),
    "uncertain or active",
  );
  await rejects(() => assertNoPendingAgent(h.context), "unresolved");
  equal(h.calls.length, 1);
});

Deno.test("a rejected launch can be discarded explicitly then dispatched with corrected effort", async () => {
  const h = harness();
  const cliCalls: string[][] = [];
  h.controls.agentRunner = (args) => {
    cliCalls.push(args);
    equal(args[3], "spawn");
    equal(args[args.indexOf("--effort") + 1], "ultra");
    return Promise.resolve({
      code: 1,
      stdout: JSON.stringify({
        outcome: "rejected",
        message: "Unsupported effort: ultra",
        agents: [],
        spawned: null,
      }),
      stderr: "",
    });
  };
  await rejects(
    () =>
      h.methods.run_agent.execute({
        ...options,
        role: "triage",
        effort: "ultra",
      }, h.context),
    "discard-agent",
  );
  const rejectedTask = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(AgentStateSchema.parse(h.child.get("state")).phase, "launch_rejected");
  equal(rejectedTask.status, "running");
  equal(rejectedTask.config.effort, "ultra");
  await rejects(
    () => h.methods.resume_agent.execute({}, h.context),
    "discard-agent",
  );
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "unresolved",
  );
  const discard = {
    request: "Discard the rejected launch so I can use supported effort",
    confirmedStopped: true as const,
  };
  for (
    const invalid of [
      { ...discard, request: " " },
      { ...discard, confirmedStopped: false },
      { request: discard.request },
    ]
  ) {
    assert(!h.methods.discard_agent.arguments.safeParse(invalid).success);
  }
  const changed = transition(createState(issue), "triage", triage);
  h.resources.set("state", changed);
  await rejects(
    () => h.methods.discard_agent.execute(discard, h.context),
    "may already be applied",
  );
  h.resources.set("state", rejectedTask.before);
  await h.methods.discard_agent.execute(discard, h.context);
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).discard, discard);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "discarded",
  );
  equal(h.resources.get("state"), rejectedTask.before);
  await assertNoPendingAgent(h.context);
  await h.methods.resume_agent.execute({}, h.context);
  equal(cliCalls.length, 1);

  // The replacement uses a fresh task identity and the explicitly corrected config.
  h.controls.agentRunner = undefined;
  await h.methods.run_agent.execute({
    ...options,
    role: "triage",
    effort: "high",
  }, h.context);
  const replacement = AgentTaskSchema.parse(h.resources.get("agent-task"));
  assert(replacement.behaviorName !== rejectedTask.behaviorName);
  equal(replacement.config.effort, "high");
  equal(replacement.status, "completed");
  equal((h.resources.get("state") as LifecycleState).phase, "classified");
});

Deno.test("resource-bearing, malformed, uncertain and legacy launch failures cannot be discarded", async () => {
  const responses = [
    { outcome: "rejected", spawned: { pane_id: "terminal_7" } },
    { outcome: "rejected", submission_id: "submission-1" },
    { outcome: "rejected", background_tasks: ["worker-1"] },
    { outcome: "rejected", launch_resources: { tab_id: 3 } },
    { outcome: "rejected", project: { name: "created", root: "/repo/new" } },
    { outcome: "rejected", restarted: { pane_id: "terminal_7" } },
    { outcome: "rejected", spawned: { pane_id: "invalid" } },
    { outcome: "failed" },
    { outcome: "timed_out" },
    "invalid JSON",
    new Error("lost spawn response"),
    // Legacy responses must not acquire the new classification on resume/discard.
    { outcome: "rejected" },
  ];
  for (const response of responses) {
    const h = harness();
    let cliCalls = 0;
    h.controls.agentRunner = () => {
      cliCalls++;
      if (response instanceof Error) return Promise.reject(response);
      return Promise.resolve({
        code: 1,
        stdout: typeof response === "string"
          ? response
          : JSON.stringify(response),
        stderr: "",
      });
    };
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
      "",
    );
    const state = AgentStateSchema.parse(h.child.get("state"));
    if (response === responses.at(-1)) {
      equal(state.phase, "launch_rejected");
      h.child.set("state", { ...state, phase: "launching" });
    } else {
      equal(state.phase, "launching");
    }
    await rejects(
      () => h.methods.resume_agent.execute({}, h.context),
      "Launch outcome is uncertain",
    );
    await rejects(
      () =>
        h.methods.discard_agent.execute({
          request: "Discard failed launch",
          confirmedStopped: true,
        }, h.context),
      "uncertain or active",
    );
    await rejects(() => assertNoPendingAgent(h.context), "unresolved");
    equal(cliCalls, 1);
    equal(h.resources.get("state"), createState(issue));
  }
});

Deno.test("implementation evidence survives an event-write failure when lifecycle state is unchanged", async () => {
  const initial = implementing();
  const h = harness(initial, {
    summary: "Implemented and checked the issue",
    commit: head,
  });
  h.controls.failEventOnce = true;
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "implement" }, h.context),
    "event write failed",
  );
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "applying",
  );
  equal(h.resources.get("state"), initial);
  await h.methods.resume_agent.execute({}, h.context);
  equal(h.calls.length, 1);
  equal(h.writes.filter(({ spec }) => spec === "event").length, 1);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "completed",
  );
});

Deno.test("resuming an applied transition after Agenda changes completes once and unblocks refresh", async () => {
  const h = harness();
  h.controls.failTaskCompletionOnce = true;
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "task completion write failed",
  );
  equal((h.resources.get("state") as LifecycleState).phase, "classified");
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "applying",
  );
  h.controls.agenda = {
    ...issue,
    note: "Revised acceptance criteria",
    digest: "e".repeat(64),
  };
  const agendaReads = h.controls.agendaReads;
  await h.methods.resume_agent.execute({}, h.context);
  equal(h.controls.agendaReads, agendaReads);
  equal(h.calls.length, 1);
  equal(h.writes.filter(({ spec }) => spec === "state").length, 1);
  equal(h.writes.filter(({ spec }) => spec === "event").length, 1);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "completed",
  );
  // Exercise refresh's pending-task guard and transition with the revised issue.
  await assertNoPendingAgent(h.context);
  await saveState(
    h.context,
    "refresh",
    refreshIssue(await loadState(h.context), h.controls.agenda),
  );
  equal((await loadState(h.context)).issue, h.controls.agenda);
  equal((await loadState(h.context)).phase, "triaging");
  equal((await loadState(h.context)).classification, undefined);
});

Deno.test("reconciliation still validates completed result identity before reading Agenda", async () => {
  const h = harness();
  h.controls.failTaskCompletionOnce = true;
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "task completion write failed",
  );
  h.child.set("result", {
    ...h.child.get("result"),
    submissionId: "another-submission",
  });
  h.controls.agenda = { ...issue, digest: "e".repeat(64) };
  const agendaReads = h.controls.agendaReads;
  await rejects(
    () => h.methods.resume_agent.execute({}, h.context),
    "correlated submission",
  );
  equal(h.controls.agendaReads, agendaReads);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "applying",
  );
  equal(h.writes.filter(({ spec }) => spec === "state").length, 1);
  equal(h.writes.filter(({ spec }) => spec === "event").length, 1);
});

Deno.test("changed requirements still block unapplied transitions and unchanged-state implementation recovery", async () => {
  for (const role of ["triage", "implement"] as const) {
    for (
      const failure of ["failEventOnce", "failTaskCompletionOnce"] as const
    ) {
      // A saved, state-changing triage transition is covered by reconciliation above.
      if (role === "triage" && failure === "failTaskCompletionOnce") continue;
      const initial = role === "implement"
        ? implementing()
        : createState(issue);
      const h = harness(
        initial,
        role === "implement"
          ? { summary: "Implemented", commit: head }
          : triage,
      );
      h.controls[failure] = true;
      await rejects(
        () => h.methods.run_agent.execute({ ...options, role }, h.context),
        "write failed",
      );
      equal(h.resources.get("state"), initial);
      const writes = h.writes.length;
      h.controls.agenda = { ...issue, digest: "e".repeat(64) };
      await rejects(
        () => h.methods.resume_agent.execute({}, h.context),
        "requirements changed",
      );
      equal(h.writes.length, writes);
      equal(h.calls.length, 1);
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).status,
        "applying",
      );
      await rejects(() => assertNoPendingAgent(h.context), "unresolved");
    }
  }
});

Deno.test("code review stays pinned to the commit dispatched to the pane", async () => {
  const changed = "c".repeat(40);
  const evidence = {
    planVersion: 1,
    commit: changed,
    steps: [{ id: "fix", status: "implemented" }],
    findings: [],
  };
  const h = harness(implementing(), evidence);
  h.controls.afterRun = () => {
    h.controls.repo.head = changed;
  };
  await rejects(
    () =>
      h.methods.run_agent.execute(
        { ...options, role: "review_code" },
        h.context,
      ),
    "different commit",
  );
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).reviewCommit,
    head,
  );
  assert(!(h.resources.get("state") as LifecycleState).codeReview);
});

Deno.test("changed Agenda requirements reject pane evidence but permit discarding a completed stale task", async () => {
  const h = harness();
  h.controls.afterRun = () => {
    h.controls.agenda = { ...issue, digest: "e".repeat(64) };
  };
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "requirements changed",
  );
  await h.methods.discard_agent.execute({
    request: "Discard completed evidence for the old requirements",
    confirmedStopped: true,
  }, h.context);
  await assertNoPendingAgent(h.context);
  equal((h.resources.get("state") as LifecycleState).phase, "triaging");
});

Deno.test("lifecycle changes during a pane assignment are not overwritten", async () => {
  const h = harness();
  const changed = transition(createState(issue), "triage", {
    classification: "maintenance",
    summary: "Another recorded investigation",
  });
  h.controls.afterRun = () => {
    h.resources.set("state", changed);
  };
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Lifecycle changed",
  );
  equal(h.resources.get("state"), changed);
  await rejects(
    () =>
      h.methods.discard_agent.execute({
        request: "Discard",
        confirmedStopped: true,
      }, h.context),
    "may already be applied",
  );
});

function recoveryRunner(failed: boolean, calls: string[][]): YinzRunner {
  return (args) => {
    calls.push(args);
    const submission = args.includes("--submission");
    const value = args[3] === "spawn"
      ? {
        outcome: "ok",
        spawned: {
          pane_id: "terminal_7",
          cwd: args[args.indexOf("--cwd") + 1],
        },
      }
      : {
        outcome: submission ? (failed ? "failed" : "completed") : "ok",
        submission_id: submission ? "submission-1" : undefined,
        message: JSON.stringify(triage),
        agents: [{
          pane_id: "terminal_7",
          session: options.session,
          agent: {
            kind: "claude",
            session_id: "conversation-1",
            state: "idle",
          },
          ready: true,
          status_available: true,
          prompt_supported: true,
        }],
      };
    return Promise.resolve({
      code: submission && failed ? 1 : 0,
      stdout: JSON.stringify(value),
      stderr: "",
    });
  };
}

Deno.test("recorded dispatch without child state recovers the original task", async () => {
  const h = harness();
  h.controls.runError = "Interrupted before initialization";
  h.controls.afterRun = () => h.child.clear();
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Interrupted",
  );
  const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const calls: string[][] = [];
  h.controls.agentRunner = recoveryRunner(false, calls);
  await h.methods.resume_agent.execute({}, h.context);
  await h.methods.resume_agent.execute({}, h.context);
  equal(h.calls[1].name, original.behaviorName);
  equal(h.calls[1].arguments, { ...original.config, prompt: original.prompt });
  equal(calls.filter((args) => args[3] === "spawn").length, 1);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "completed",
  );
  await assertNoPendingAgent(h.context);
});

Deno.test("terminal provider failure can be explicitly discarded and unblocks lifecycle", async () => {
  const h = harness();
  const calls: string[][] = [];
  h.controls.agentRunner = recoveryRunner(true, calls);
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "has not completed",
  );
  const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
  await rejects(() => assertNoPendingAgent(h.context), "unresolved");
  await rejects(
    () =>
      h.methods.discard_agent.execute({
        request: "Discard",
        confirmedStopped: false as true,
      }, h.context),
    "true",
  );
  const discard = {
    request: "Discard the stopped provider failure",
    confirmedStopped: true as const,
  };
  h.resources.set("state", transition(original.before, "triage", triage));
  await rejects(
    () => h.methods.discard_agent.execute(discard, h.context),
    "may already be applied",
  );
  h.resources.set("state", original.before);
  await h.methods.discard_agent.execute(discard, h.context);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "discarded",
  );
  equal(h.resources.get("state"), original.before);
  assert(!h.child.has("result"));
  await assertNoPendingAgent(h.context);
  h.controls.agentRunner = undefined;
  await h.methods.run_agent.execute({ ...options, role: "triage" }, h.context);
  equal((h.resources.get("state") as LifecycleState).phase, "classified");
});
