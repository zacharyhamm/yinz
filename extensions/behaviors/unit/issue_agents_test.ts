import {
  AgentTaskSchema,
  assertNoPendingAgent,
  createAgentMethods,
  type IssueAgentContext,
  type IssueAgentDependencies,
} from "../_lib/issue_agents.ts";
import {
  createDriveMethod,
  DriverSchema,
  HandoffSchema,
} from "../_lib/issue_driver.ts";
import {
  createState,
  effectivePlanFindings,
  initialPlanning,
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
  control: { session: "development", paneId: "terminal_1" },
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
    checks: [{
      id: "focused",
      command: ["cargo", "x", "test", "-p", "demo"],
      covers: ["fix"],
    }],
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

function verify(state: LifecycleState): LifecycleState {
  const id = "c6da10b0-c980-4b82-b920-640238095f1a";
  return transition(state, "verify", {
    verification: {
      id,
      planVersion: state.plan!.version,
      commit: state.codeReview!.commit,
      branch: state.implementation!.branch,
      startedAt: timestamp,
      finishedAt: timestamp,
      status: "passed",
      checks: state.plan!.checks!.map((check) => ({
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
  let childBehavior: string | undefined;
  let issueLocked = false;
  const controls = {
    cleanupCalls: 0,
    cleanupError: false,
    evidence,
    evidenceForRole: undefined as
      | ((task: ReturnType<typeof AgentTaskSchema.parse>) => unknown)
      | undefined,
    agenda: issue,
    failEventOnce: false,
    failTaskCompletionOnce: false,
    failWrite: undefined as
      | ((
        spec: string,
        name: string,
        value: Record<string, unknown>,
      ) => boolean)
      | undefined,
    childPhase: "completed",
    childOutcome: "completed",
    childBackground: [] as string[],
    childPatch: {} as Record<string, unknown>,
    resultPatch: {} as Record<string, unknown>,
    runError: undefined as string | undefined,
    reopenError: undefined as string | undefined,
    reopened: 0,
    receipt: { status: "missing" } as unknown,
    checkCalls: [] as {
      request: Record<string, unknown>;
      log?: string;
      spec?: string;
    }[],
    checkOutcome: (_request: Record<string, unknown>) =>
      ({ exitCode: 0, durationMs: 1200, worktreeUnchanged: true }) as {
        exitCode: number | null;
        durationMs: number;
        worktreeUnchanged: boolean;
      },
    agentRunner: undefined as YinzRunner | undefined,
    agendaReads: 0,
    controlTab: 0,
    placementCalls: [] as string[][],
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
      if (controls.failWrite?.(spec, name, value)) {
        controls.failWrite = undefined;
        return Promise.reject(new Error("Interrupted recovery write"));
      }
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
      if (request.method === "cleanup") {
        const task = AgentTaskSchema.parse(resources.get("agent-task"));
        equal(task.status, "completed");
        equal(task.cleanup, "pending");
        controls.cleanupCalls++;
        return controls.cleanupError
          ? { ok: false, error: { message: "cleanup unavailable" } }
          : { ok: true };
      }
      calls.push(request);
      const fresh = request.name !== undefined &&
        request.name !== childBehavior;
      if (fresh) {
        child.clear();
        childBehavior = request.name;
      }
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
          run: (args: string[], stdin?: string, signal?: AbortSignal) => {
            if (args[3] === "list-panes") {
              controls.placementCalls.push(args);
              return Promise.resolve({
                code: 0,
                stdout: JSON.stringify([{
                  id: 1,
                  is_plugin: false,
                  tab_id: controls.controlTab,
                }]),
                stderr: "",
              });
            }
            return controls.agentRunner!(args, stdin, signal);
          },
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
      if (request.method === "run" || (request.method === "recover" && fresh)) {
        const task = AgentTaskSchema.parse(resources.get("agent-task"));
        equal(request.name, task.behaviorName);
        const { prompt, ...rawConfig } = request.arguments!;
        assert(typeof prompt === "string");
        const config = AgentConfigSchema.parse(rawConfig);
        const digest = await promptDigest(prompt);
        const evidence = controls.evidenceForRole?.(task) ?? controls.evidence;
        const message = typeof evidence === "string"
          ? evidence
          : JSON.stringify(evidence);
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
    withIssue: async (_context, operation) => {
      assert(!issueLocked, "Nested issue lock");
      issueLocked = true;
      try {
        return await operation();
      } finally {
        issueLocked = false;
      }
    },
    readAgenda: () => {
      controls.agendaReads++;
      return Promise.resolve(controls.agenda);
    },
    inspect: () => Promise.resolve(controls.repo),
    gitOperation: (_context, request, log, spec) => {
      if (request.action !== "check") return Promise.resolve(controls.receipt);
      controls.checkCalls.push({ request, log, spec });
      return Promise.resolve(controls.checkOutcome(request));
    },
    reopenAgenda: () => {
      controls.reopened++;
      return controls.reopenError
        ? Promise.reject(new Error(controls.reopenError))
        : Promise.resolve();
    },
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
    drive: createDriveMethod(dependencies),
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
        acknowledged: submission,
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

Deno.test("explicit unsent discard waits for drained writes, preserves evidence and lifecycle, and is idempotent", async () => {
  const h = harness();
  const calls: string[][] = [];
  const run = recoveryRunner(true, calls);
  let pendingWrites = true;
  h.controls.agentRunner = async (args) => {
    const reply = await run(args);
    const value = JSON.parse(reply.stdout);
    if (args.includes("--submission")) {
      value.outcome = "target_changed";
      value.prompt_delivery = "not_sent";
      value.acknowledged = false;
      value.turn_id = null;
    }
    if (args[3] === "status") {
      value.agents[0].ready = false;
      value.agents[0].not_ready_reason = pendingWrites
        ? "clearing_draft"
        : "unconsumed_input";
    }
    return { ...reply, stdout: JSON.stringify(value) };
  };
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "has not completed",
  );
  const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const discard = {
    request: "Discard the task proven unsent; inspect the marker before reuse",
    confirmedStopped: true as const,
  };
  await rejects(
    () => h.methods.discard_agent.execute(discard, h.context),
    "pending clearing/submission writes",
  );
  await rejects(() => assertNoPendingAgent(h.context), "unresolved");
  pendingWrites = false;
  h.resources.set("state", transition(original.before, "triage", triage));
  await rejects(
    () => h.methods.discard_agent.execute(discard, h.context),
    "may already be applied",
  );
  h.resources.set("state", original.before);
  const evidence = h.child.get("state")?.rawResponse;
  await h.methods.discard_agent.execute(discard, h.context);
  const callCount = calls.length;
  await h.methods.discard_agent.execute(discard, h.context);
  equal(calls.length, callCount);
  equal(h.resources.get("state"), original.before);
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).discard, discard);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "discarded",
  );
  equal(h.child.get("state")?.rawResponse, evidence);
  equal(h.child.get("observation")?.outcome, "ok");
  assert(!h.child.has("result"));
  equal(calls.filter((args) => args[3] === "spawn").length, 1);
  equal(calls.filter((args) => args[3] === "prompt").length, 1);
  await assertNoPendingAgent(h.context);
});

Deno.test("legacy stopped preparation failures remain blocked without server non-delivery evidence", async () => {
  for (
    const outcome of [
      "target_changed",
      "status_unavailable",
      "failed",
      "interrupted",
      "timed_out",
    ]
  ) {
    const h = harness();
    const calls: string[][] = [];
    const run = recoveryRunner(true, calls);
    h.controls.agentRunner = async (args) => {
      const reply = await run(args);
      const value = JSON.parse(reply.stdout);
      if (args.includes("--submission")) {
        value.outcome = outcome;
        value.acknowledged = false;
        value.message = "agent identity changed; prompt was not sent";
      }
      return { ...reply, stdout: JSON.stringify(value) };
    };
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
      "has not completed",
    );
    const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
    const count = calls.length;
    await rejects(
      () =>
        h.methods.discard_agent.execute({
          request: "Discard stopped task",
          confirmedStopped: true,
        }, h.context),
      "legacy text is insufficient",
    );
    equal(calls.length, count);
    equal(h.resources.get("state"), original.before);
    equal(h.resources.get("agent-task"), original);
    await rejects(() => assertNoPendingAgent(h.context), "unresolved");
  }
});

Deno.test("explicit restart supersedes every worker outcome without querying or discarding the old child", async () => {
  for (
    const failure of [
      "timed_out",
      "failed",
      "target_changed",
      "transport_error",
      "rejected",
      "completed",
      "missing",
    ]
  ) {
    const initial = transition(createState(issue), "triage", triage);
    const evidence = {
      summary: "New plan",
      steps: [{ id: "fix", description: "Fix it", files: [] }],
      testingStrategy: "Focused tests",
      checks: [{
        id: "focused",
        command: ["cargo", "x", "test", "-p", "demo"],
        covers: ["fix"],
      }],
    };
    const h = harness(initial, evidence);
    h.controls.childPhase = "waiting";
    h.controls.childOutcome = failure;
    h.controls.runError = "Old attempt failed";
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "plan" }, h.context),
      "Old attempt failed",
    );
    const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
    if (failure === "missing") h.child.clear();
    h.controls.childPhase = "completed";
    h.controls.childOutcome = "completed";
    h.controls.runError = undefined;
    const args = {
      ...options,
      role: "plan" as const,
      session: "replacement-session",
      control: { session: "replacement-session", paneId: "terminal_1" },
      taskId: old.id,
      request: "Restart planning",
    };
    await h.methods.restart_agent.execute(args, h.context);
    const next = AgentTaskSchema.parse(h.resources.get("agent-task"));
    assert(next.id !== old.id && next.behaviorName !== old.behaviorName);
    equal(next.config.session, "replacement-session");
    equal(next.status, "completed");
    equal(h.calls.length, 2);
    equal(h.calls[1].method, "recover");
    equal((await loadState(h.context)).plan?.version, 1);
    equal(
      AgentTaskSchema.parse(h.resources.get(`agent-task-${old.id}`)).status,
      "superseded",
    );
    const recovery = h.resources.get("recovery") as {
      task: unknown;
      before: unknown;
    };
    equal(recovery.task, old);
    equal(recovery.before, initial);
    // Retrying a request after a lost response observes the replacement, never spawns a third worker.
    await h.methods.restart_agent.execute(args, h.context);
    equal(h.calls.length, 2);
    equal((await loadState(h.context)).plan?.version, 1);
    await rejects(
      () =>
        h.methods.restart_agent.execute(
          { ...args, session: "changed" },
          h.context,
        ),
      "current assignment",
    );
  }
});

Deno.test("rollback fences old results even when returning to the same lifecycle phase", async () => {
  const h = harness();
  h.controls.runError = "lost response";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "lost response",
  );
  const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
  await h.methods.rollback.execute({
    step: "triage",
    request: "Restart triage",
  }, h.context);
  const state = await loadState(h.context);
  equal(state.phase, "triaging");
  assert(state.rollbackId !== undefined);
  await h.methods.resume_agent.execute({}, h.context);
  equal(h.calls.length, 1);
  equal(await loadState(h.context), state);
  // Even if an old task snapshot is accidentally restored, the generation fence rejects its result.
  h.resources.set("agent-task", old);
  h.controls.runError = undefined;
  await rejects(
    () => h.methods.resume_agent.execute({}, h.context),
    "Lifecycle changed",
  );
  equal(await loadState(h.context), state);
});

Deno.test("rollback keeps applied evidence in its history and reopens close-out", async () => {
  const h = harness();
  h.controls.failTaskCompletionOnce = true;
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "completion write failed",
  );
  const applied = await loadState(h.context);
  await h.methods.rollback.execute(
    { step: "triage", request: "Start again" },
    h.context,
  );
  equal((h.resources.get("recovery") as { before: unknown }).before, applied);
  equal((await loadState(h.context)).classification, undefined);
  equal(h.controls.reopened, 1);
  await assertNoPendingAgent(h.context);
});

Deno.test("interrupted rollback blocks other mutations and resumes without requiring the old worker", async () => {
  const h = harness(createState(issue, "/repo/origin"));
  h.controls.runError = "session disappeared";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "session disappeared",
  );
  const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(old.config.cwd, "/repo/origin");
  h.context.repoDir = "/repo/later-invocation";
  const args = {
    ...options,
    role: "triage" as const,
    taskId: old.id,
    request: "Restart",
  };
  h.controls.reopenError = "Agenda unavailable";
  await rejects(
    () => h.methods.restart_agent.execute(args, h.context),
    "Agenda unavailable",
  );
  equal(h.calls.length, 1);
  await rejects(
    () => h.methods.resume_agent.execute({}, h.context),
    "resume-rollback",
  );
  await rejects(() => assertNoPendingAgent(h.context), "resume-rollback");
  h.child.clear();
  h.controls.reopenError = undefined;
  h.controls.runError = undefined;
  h.controls.failEventOnce = true;
  await rejects(
    () => h.methods.resume_rollback.execute({}, h.context),
    "event write failed",
  );
  await h.methods.resume_rollback.execute({}, h.context);
  equal(h.calls.length, 2);
  equal(h.calls.map((call) => call.arguments?.cwd), [
    "/repo/origin",
    "/repo/origin",
  ]);
  equal((await loadState(h.context)).workerWorktree, "/repo/origin");
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "completed",
  );
  await h.methods.resume_rollback.execute({}, h.context);
  equal(h.calls.length, 2);
});

Deno.test("replacement failure remains resumable and a stale restart request cannot replace a newer assignment", async () => {
  const h = harness();
  h.controls.runError = "provider unavailable";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "provider unavailable",
  );
  const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const args = {
    ...options,
    role: "triage" as const,
    taskId: old.id,
    request: "Restart",
  };
  await rejects(
    () => h.methods.restart_agent.execute(args, h.context),
    "provider unavailable",
  );
  const replacement = AgentTaskSchema.parse(h.resources.get("agent-task"));
  assert(replacement.id !== old.id);
  equal(replacement.lastError, "provider unavailable");
  h.controls.runError = undefined;
  await h.methods.restart_agent.execute(args, h.context);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).id,
    replacement.id,
  );
  await h.methods.run_agent.execute({ ...options, role: "triage" }, h.context);
  const calls = h.calls.length;
  await rejects(
    () => h.methods.restart_agent.execute(args, h.context),
    "superseded",
  );
  equal(h.calls.length, calls);
});

Deno.test("rollback validates prerequisites and refreshed requirements before superseding a task", async () => {
  const h = harness();
  await h.methods.run_agent.execute({ ...options, role: "triage" }, h.context);
  const before = structuredClone([...h.resources]);
  await rejects(
    () =>
      h.methods.rollback.execute(
        { step: "merge", request: "Retry" },
        h.context,
      ),
    "missing plan evidence",
  );
  equal([...h.resources], before);
  h.controls.agenda = {
    ...issue,
    note: "New requirements",
    digest: "e".repeat(64),
  };
  await rejects(
    () =>
      h.methods.rollback.execute({ step: "plan", request: "Retry" }, h.context),
    "roll back to triage",
  );
  equal([...h.resources], before);
  await h.methods.rollback.execute({
    step: "triage",
    request: "Start with the new requirements",
  }, h.context);
  equal((await loadState(h.context)).issue, h.controls.agenda);
  assert(
    !h.methods.rollback.arguments.safeParse({ step: "plan", request: " " })
      .success,
  );
});

Deno.test("every rollback persistence boundary can be resumed without duplicate dispatch", async () => {
  for (
    const boundary of [
      "archive",
      "state",
      "replacement",
      "history",
      "completion",
    ]
  ) {
    const h = harness();
    h.controls.runError = "lost old worker";
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
      "lost old worker",
    );
    const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
    h.controls.runError = undefined;
    h.controls.failWrite = (spec, name, value) => {
      switch (boundary) {
        case "archive":
          return name === `agent-task-${old.id}`;
        case "state":
          return spec === "state";
        case "replacement":
          return name === "agent-task" && value.id !== old.id;
        case "history":
          return spec === "recovery" && name !== "recovery";
        case "completion":
          return name === "recovery" && value.status === "completed";
        default:
          return false;
      }
    };
    const args = {
      ...options,
      role: "triage" as const,
      taskId: old.id,
      request: "Restart",
    };
    await rejects(
      () => h.methods.restart_agent.execute(args, h.context),
      "Interrupted recovery write",
    );
    equal(h.calls.length, 1);
    await rejects(() => assertNoPendingAgent(h.context), "resume-rollback");
    await h.methods.restart_agent.execute(args, h.context);
    await h.methods.resume_rollback.execute({}, h.context);
    equal(h.calls.length, 2);
    equal((await loadState(h.context)).phase, "classified");
    equal(
      AgentTaskSchema.parse(h.resources.get("agent-task")).status,
      "completed",
    );
  }
});

Deno.test("rollback reconciles a landed merge before clearing an interrupted attempt", async () => {
  let state = transition(implementing(), "review_code", {
    planVersion: 1,
    commit: head,
    steps: [{ id: "fix", status: "implemented" }],
    findings: [],
  });
  state = transition(verify(state), "ready", {
    commit: head,
    mergeRequest: "Merge",
  });
  const attempt = {
    id: "a6da10b0-c980-4b82-b920-640238095f1b",
    originalHead: head,
    startedAt: timestamp,
  };
  state = transition(state, "begin_merge", { attempt });
  for (const step of ["triage", "plan", "merge", "summarize"] as const) {
    const h = harness(state);
    h.controls.receipt = {
      status: "merged",
      attemptId: attempt.id,
      originalCommit: head,
      rebasedCommit: "c".repeat(40),
      mergedCommit: "c".repeat(40),
      baseCommit: "b".repeat(40),
      worktree: "/repo/issue",
      branch: "issue-85",
      mergedAt: timestamp,
      issueKey: "default/85",
      issueDigest: issue.digest,
      ci: { status: "passed", command: ["cargo", "x", "ci", "check"] },
    };
    await h.methods.rollback.execute(
      { step, request: "Restart this step" },
      h.context,
    );
    const next = await loadState(h.context);
    assert(!next.attempt);
    if (step === "merge" || step === "summarize") {
      equal(next.phase, "merged");
      equal(next.receipt?.attemptId, attempt.id);
    } else {
      assert(!next.receipt);
    }
    const history = h.resources.get("recovery") as {
      reconciled: LifecycleState;
    };
    equal(history.reconciled.receipt?.attemptId, attempt.id);
  }
});

Deno.test("a new explicit rollback supersedes an interrupted recovery including stale requirements", async () => {
  const h = harness(transition(createState(issue), "triage", triage));
  h.controls.reopenError = "Agenda offline";
  await rejects(
    () =>
      h.methods.rollback.execute(
        { step: "plan", request: "Retry planning" },
        h.context,
      ),
    "Agenda offline",
  );
  const pending = h.resources.get("recovery") as { id: string };
  h.controls.agenda = {
    ...issue,
    note: "Changed while offline",
    digest: "e".repeat(64),
  };
  h.controls.reopenError = undefined;
  await h.methods.rollback.execute({
    step: "triage",
    request: "Start over with the new requirements",
  }, h.context);
  const archived = h.resources.get(`recovery-${pending.id}`) as {
    status: string;
    supersededBy: string;
  };
  equal(archived.status, "superseded");
  equal(archived.supersededBy, (await loadState(h.context)).rollbackId);
  equal((await loadState(h.context)).issue, h.controls.agenda);
  await assertNoPendingAgent(h.context);
  await h.methods.resume_rollback.execute({}, h.context);
  equal(h.calls.length, 0);
});

Deno.test("a new restart can change options while an earlier restart has not saved its replacement", async () => {
  const h = harness();
  h.controls.runError = "missing session";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "missing session",
  );
  const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const args = {
    ...options,
    role: "triage" as const,
    taskId: old.id,
    request: "Restart",
  };
  h.controls.reopenError = "Agenda offline";
  await rejects(
    () => h.methods.restart_agent.execute(args, h.context),
    "Agenda offline",
  );
  const pending = h.resources.get("recovery") as { id: string };
  h.controls.reopenError = undefined;
  h.controls.runError = undefined;
  await h.methods.restart_agent.execute({
    ...args,
    session: "new-session",
    control: { session: "new-session", paneId: "terminal_1" },
    request: "Restart in the new session",
  }, h.context);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).config.session,
    "new-session",
  );
  equal(
    (h.resources.get(`recovery-${pending.id}`) as { status: string }).status,
    "superseded",
  );
  equal(h.calls.length, 2);
});

const driverPlan = {
  summary: "Repair issue behavior",
  steps: [{ id: "fix", description: "Repair and test", files: ["src/lib.rs"] }],
  testingStrategy: "Focused regression and affected caller tests",
  assumptions: ["The recorded reproduction covers the affected caller"],
  checks: [
    {
      id: "regression",
      command: ["cargo", "x", "test", "-p", "demo", "-E", "test(/^fix::/)"],
      covers: ["fix"],
    },
    {
      id: "caller",
      command: ["cargo", "x", "test", "caller"],
      covers: ["fix"],
    },
  ],
};
function driverImplementing(): LifecycleState {
  let state = transition(createState(issue), "triage", triage);
  state = transition(state, "plan", driverPlan);
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
function driverEvidence(
  task: ReturnType<typeof AgentTaskSchema.parse>,
): unknown {
  switch (task.role) {
    case "triage":
      return triage;
    case "plan":
      return initialPlanning(task.before) ? driverPlan : {
        ...driverPlan,
        revision: {
          summary: "Targeted caller coverage changes",
          responses: (task.before.planReview?.findings ??
            task.before.planningContext?.history.findLast((h) => h.review)
              ?.review?.findings ??
            [])
            .filter((f) =>
              ["high", "critical"].includes(f.severity) && !f.resolution
            )
            .map((f) => ({
              findingId: f.id,
              status: "disputed",
              rationale: "Caller is covered",
              evidence: ["src/lib.rs:42"],
            })),
          acceptedDecisions: ["Keep the existing caller API"],
        },
      };
    case "review_plan":
      return { planVersion: task.before.plan!.version, findings: [] };
    case "implement":
      return {
        summary: "Repaired the affected caller and added a regression",
        commit: head,
      };
    case "review_code":
      return {
        planVersion: task.before.plan!.version,
        commit: head,
        steps: [{ id: "fix", status: "implemented" }],
        findings: [],
        preliminaryChecks: [{
          command: "focused regression",
          result: "passed",
          summary: "Affected caller regression passes",
        }],
      };
  }
}
function dispatchedRoles(h: ReturnType<typeof harness>) {
  return h.writes.filter((write) =>
    write.spec === "agent-task" && write.value.status === "running" &&
    !write.value.lastError
  )
    .map((write) => write.value.role);
}
function result(h: ReturnType<typeof harness>) {
  return HandoffSchema.parse(h.resources.get("handoff"));
}

Deno.test("driver takes planning through fresh review to a reusable approval handoff without implementing", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  const output = await h.drive.execute({ agent: options }, h.context);
  equal(
    new Set(output.dataHandles.map((handle) => handle.name)).size,
    output.dataHandles.length,
  );
  equal(dispatchedRoles(h), ["triage", "plan", "review_plan"]);
  const handoff = result(h);
  equal(handoff.decision, "approve_plan");
  equal(handoff.planVersion, 1);
  equal(handoff.state.plan?.assumptions, driverPlan.assumptions);
  for (
    const text of [
      "Proposed behavior",
      "Implementation steps",
      "Tests:",
      "Assumptions:",
      "Plan review findings",
      "controlling agent must collect and present",
    ]
  ) {
    assert(handoff.presentation.includes(text));
  }
  equal((h.resources.get("driver") as { agent: unknown }).agent, options);
  await h.drive.execute({}, h.context);
  equal(h.calls.length, 3);
  equal(result(h).id, handoff.id);
  equal((await loadState(h.context)).approval, undefined);
});

function blockedPlanEvidence(task: ReturnType<typeof AgentTaskSchema.parse>) {
  return task.role === "review_plan"
    ? {
      planVersion: task.before.plan!.version,
      findings: [{
        id: "gap",
        severity: "critical",
        description: "Cover the caller",
        requirement: "Caller coverage",
        mechanism: "Caller can bypass validation",
        evidence: ["src/lib.rs:42"],
      }],
    }
    : driverEvidence(task);
}
function revisionApproval(h: ReturnType<typeof harness>) {
  return {
    handoffId: result(h).id,
    request: "Revise this plan once to address the findings",
  };
}

Deno.test("driver automatically revises once then stops at a stable revision proposal and one authorization permits only one revision and review", async () => {
  const h = harness();
  h.controls.evidenceForRole = blockedPlanEvidence;
  await h.drive.execute({ agent: options }, h.context);
  const proposal = result(h);
  equal(proposal.decision, "authorize_plan_revision");
  equal(proposal.planVersion, 2);
  equal(proposal.proposedPlanVersion, 3);
  assert(proposal.presentation.includes("Cover the caller"));
  assert(proposal.presentation.includes("Proposed next plan version: 3"));
  assert(
    proposal.presentation.includes("does not approve implementation or merge"),
  );
  equal(dispatchedRoles(h), [
    "triage",
    "plan",
    "review_plan",
    "plan",
    "review_plan",
  ]);
  await h.drive.execute({}, h.context);
  equal(result(h).id, proposal.id);
  equal(h.calls.length, 5);
  const approval = revisionApproval(h);
  await h.drive.execute({ revisionApproval: approval }, h.context);
  equal(dispatchedRoles(h), [
    "triage",
    "plan",
    "review_plan",
    "plan",
    "review_plan",
    "plan",
    "review_plan",
  ]);
  equal(result(h).decision, "authorize_plan_revision");
  equal(result(h).planVersion, 3);
  equal(result(h).proposedPlanVersion, 4);
  assert(result(h).id !== proposal.id);
  assert(h.calls[3].arguments!.prompt!.toString().includes("Cover the caller"));
  const authorization = DriverSchema.parse(h.resources.get("driver"))
    .revisionAuthorization!;
  equal(authorization.request, approval.request);
  assert(authorization.assignment.prompt.includes(approval.request));
  assert(
    authorization.assignment.prompt.includes(
      "Targeted caller coverage changes",
    ),
  );
  equal(authorization.assignment.before, proposal.state);
  const saved = h.writes.findIndex((write) =>
    write.spec === "driver" && !!write.value.revisionAuthorization
  );
  const dispatched = h.writes.findIndex((write) =>
    write.spec === "agent-task" &&
    write.value.id === authorization.assignment.id
  );
  assert(saved < dispatched);
  const nextProposal = result(h);
  await h.drive.execute({}, h.context);
  equal(result(h).id, nextProposal.id);
  equal(h.calls.length, 7);
  await h.drive.execute({ revisionApproval: approval }, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("stale"));
  equal(h.calls.length, 7);
  await h.drive.execute({}, h.context);
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ revisionApproval: revisionApproval(h) }, h.context);
  equal(result(h).decision, "approve_plan");
  equal(result(h).planVersion, 4);
  equal((await loadState(h.context)).approval, undefined);
  equal((await loadState(h.context)).mergeAuthorization, undefined);
});

Deno.test("revision approval rejects blank requests and other handoff types without dispatch", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  const approval = revisionApproval(h);
  await rejects(
    () =>
      h.drive.execute(
        { revisionApproval: { ...approval, request: "  " } },
        h.context,
      ),
    "Too small",
  );
  equal(result(h).decision, "approve_plan");
  await h.drive.execute({ revisionApproval: approval }, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("wrong handoff type"));
  equal(h.calls.length, 3);
});

for (const fault of ["dispatch", "worker", "event", "completion"]) {
  Deno.test(`authorized revision recovers interrupted ${fault} using the exact assignment`, async () => {
    const h = harness();
    h.controls.evidenceForRole = blockedPlanEvidence;
    await h.drive.execute({ agent: options }, h.context);
    const approval = revisionApproval(h);
    if (fault === "dispatch") {
      h.controls.failWrite = (spec, _name, value) =>
        spec === "agent-task" && value.status === "running";
    }
    if (fault === "worker") {
      h.controls.runError = "Interrupted worker observation";
    }
    if (fault === "event") h.controls.failEventOnce = true;
    if (fault === "completion") h.controls.failTaskCompletionOnce = true;
    await h.drive.execute({
      agent: { ...options, model: "revision-model" },
      revisionApproval: approval,
    }, h.context);
    equal(result(h).decision, "blocked");
    const authorization = DriverSchema.parse(h.resources.get("driver"))
      .revisionAuthorization!;
    h.controls.runError = undefined;
    // Both an input retry and ordinary resume reuse the recorded authorization.
    await h.drive.execute(
      fault === "dispatch" || fault === "completion"
        ? { revisionApproval: approval }
        : {},
      h.context,
    );
    equal(result(h).decision, "authorize_plan_revision");
    equal(result(h).planVersion, 3);
    assert(
      h.calls.slice(5).every((call) =>
        call.arguments?.model === "revision-model"
      ),
    );
    const planners = h.calls.slice(5).filter((call) =>
      call.name === authorization.assignment.behaviorName
    );
    equal(planners.filter((call) => call.method === "run").length, 1);
    equal(
      planners.filter((call) => call.method === "recover").length,
      fault === "worker" ? 1 : 0,
    );
    equal(
      DriverSchema.parse(h.resources.get("driver")).revisionAuthorization!
        .assignment.id,
      authorization.assignment.id,
    );
    const count = h.calls.length;
    await h.drive.execute({}, h.context);
    equal(h.calls.length, count);
  });
}

for (const change of ["requirements", "rollback", "manual_plan"]) {
  for (const authorized of [false, true]) {
    Deno.test(`${change} invalidates ${authorized ? "persisted authorization" : "revision proposal"} and gates the next automatic plan`, async () => {
      const h = harness();
      h.controls.evidenceForRole = blockedPlanEvidence;
      await h.drive.execute({ agent: options }, h.context);
      const approval = revisionApproval(h);
      if (authorized) {
        h.controls.failWrite = (spec, _name, value) =>
          spec === "agent-task" && value.status === "running";
        await h.drive.execute({ revisionApproval: approval }, h.context);
      }
      if (change === "requirements") {
        h.controls.agenda = {
          ...issue,
          digest: "e".repeat(64),
          note: "Changed requirements",
        };
        await h.drive.execute({ revisionApproval: approval }, h.context);
        equal(result(h).decision, "blocked");
        assert(result(h).reason.includes("requirements changed"));
        h.resources.set(
          "state",
          refreshIssue(await loadState(h.context), h.controls.agenda),
        );
      } else if (change === "rollback") {
        await h.methods.rollback.execute({
          step: "plan",
          request: "Return to planning",
        }, h.context);
      } else {
        await saveState(
          h.context,
          "plan",
          transition(await loadState(h.context), "plan", driverPlan),
        );
      }
      await h.drive.execute({ revisionApproval: approval }, h.context);
      equal(result(h).decision, "blocked");
      equal(h.calls.length, 5);
      await h.drive.execute({}, h.context);
      equal(result(h).decision, "authorize_plan_revision");
      equal(
        result(h).proposedPlanVersion,
        change === "requirements" ? 5 : change === "manual_plan" ? 4 : 3,
      );
      equal(
        dispatchedRoles(h).filter((role) => role === "plan").length,
        change === "requirements" ? 4 : 2,
      );
      assert(result(h).id !== approval.handoffId);
      // The new proposal can authorize exactly the next version.
      await h.drive.execute(
        { revisionApproval: revisionApproval(h) },
        h.context,
      );
      equal(
        result(h).planVersion,
        change === "requirements" ? 5 : change === "manual_plan" ? 4 : 3,
      );
    });
  }
}

Deno.test("manual planning and planner restart remain ungated, and legacy running planners can finish", async () => {
  const h = harness();
  h.controls.evidenceForRole = blockedPlanEvidence;
  await h.drive.execute({ agent: options }, h.context);
  await h.methods.run_agent.execute({ ...options, role: "plan" }, h.context);
  equal((await loadState(h.context)).planVersion, 3);
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  await h.methods.restart_agent.execute({
    ...options,
    role: "plan",
    taskId: task.id,
    request: "Restart planning",
  }, h.context);
  equal((await loadState(h.context)).planVersion, 4);
  h.controls.failEventOnce = true;
  await rejects(
    () => h.methods.run_agent.execute({ ...options, role: "plan" }, h.context),
    "Fixture event write failed",
  );
  equal(
    DriverSchema.parse(h.resources.get("driver")).revisionAuthorization,
    undefined,
  );
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_plan_revision");
  equal(result(h).planVersion, 5);
  equal(result(h).proposedPlanVersion, 6);
});

Deno.test("driver requires worktree preparation after explicit approval, then implements and reviews to merge authorization", async () => {
  const initial = implementing();
  const approved = {
    ...initial,
    phase: "approved" as const,
    implementation: undefined,
  };
  const h = harness(approved);
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "prepare_worktree");
  equal(h.calls.length, 0);
  h.resources.set("state", initial);
  await h.drive.execute({}, h.context);
  equal(dispatchedRoles(h), ["implement", "review_code"]);
  equal(result(h).decision, "authorize_merge");
  equal(result(h).commit, head);
  assert(result(h).presentation.includes("Repaired the affected caller"));
  assert(result(h).presentation.includes("focused regression: passed"));
  equal((await loadState(h.context)).mergeAuthorization, undefined);
  const id = result(h).id;
  await h.drive.execute({}, h.context);
  equal(result(h).id, id);
  equal(h.calls.length, 2);
});

Deno.test("driver returns code review blockers instead of running checks or requesting merge authorization", async () => {
  const h = harness(implementing());
  h.controls.evidenceForRole = (task) =>
    task.role === "review_code"
      ? {
        ...driverEvidence(task) as object,
        findings: [{
          id: "bug",
          severity: "high",
          description: "Race remains",
        }],
      }
      : driverEvidence(task);
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("code review blockers"));
  equal(h.controls.checkCalls.length, 0);
});

Deno.test("driver runs every plan check at the reviewed commit and presents a checklist from the run", async () => {
  const h = harness(driverImplementing());
  // Reviewer-reported failures are advisory once the plan has checks.
  h.controls.evidenceForRole = (task) =>
    task.role === "review_code"
      ? {
        ...driverEvidence(task) as object,
        preliminaryChecks: [{
          command: "regression",
          result: "failed",
          summary: "Reviewer saw a failure",
        }],
      }
      : driverEvidence(task);
  h.controls.checkOutcome = (request) => ({
    exitCode: 0,
    durationMs: (request.argv as string[]).length * 1000,
    worktreeUnchanged: true,
  });
  await h.drive.execute({ agent: options }, h.context);
  const handoff = result(h);
  equal(handoff.decision, "authorize_merge");
  const state = await loadState(h.context);
  const verification = state.verification!;
  equal(verification.status, "passed");
  equal(verification.commit, head);
  equal(verification.planVersion, 1);
  equal(
    h.controls.checkCalls.map(({ request, log, spec }) => ({
      request,
      log,
      spec,
    })),
    driverPlan.checks.map((check) => ({
      request: {
        action: "check",
        worktree: "/repo/issue",
        expectedBranch: "issue-85",
        expectedHead: head,
        argv: check.command,
      },
      log: `check-log-${verification.id}-${check.id}`,
      spec: "check-log",
    })),
  );
  for (
    const text of [
      `Verification ${verification.id}: passed at ${head} (plan version 1)`,
      "- [x] regression: cargo x test -p demo -E 'test(/^fix::/)' -> exit 0 in 7.0s (covers fix).",
      "- [x] caller: cargo x test caller -> exit 0 in 4.0s (covers fix).",
      `Log: swamp data query 'modelName == "issue-default-85" && name == "check-log-${verification.id}-regression"' --select content`,
      "Reviewer-reported checks (advisory):",
      "- regression: failed. Reviewer saw a failure",
      "Checks (run verbatim in the issue worktree before merge authorization):",
    ]
  ) assert(handoff.presentation.includes(text), text);
  // A resumed drive reuses the verification instead of running checks again.
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_merge");
  equal(h.controls.checkCalls.length, 2);
});

Deno.test("driver blocks on a failed check, names its log, and never reruns it automatically", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  h.controls.checkOutcome = (request) => ({
    exitCode: (request.argv as string[]).includes("caller") ? 101 : 0,
    durationMs: 50,
    worktreeUnchanged: true,
  });
  await h.drive.execute({ agent: options }, h.context);
  const handoff = result(h);
  equal(handoff.decision, "blocked");
  const verification = (await loadState(h.context)).verification!;
  equal(verification.status, "failed");
  assert(handoff.reason.includes(`Verification ${verification.id} failed`));
  assert(
    handoff.reason.includes(
      `caller (exit 101; log: swamp data query 'modelName == "issue-default-85" && name == "check-log-${verification.id}-caller"' --select content)`,
    ),
    handoff.reason,
  );
  assert(!handoff.reason.includes("regression ("));
  assert(handoff.presentation.includes("- [ ] caller:"));
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  equal(h.controls.checkCalls.length, 2);
});

Deno.test("a check that changes the worktree fails and later checks are recorded as not run", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  h.controls.checkOutcome = () => {
    // The real worktree is left dirty, so the review would otherwise look stale.
    h.controls.repo = { ...h.controls.repo, clean: false };
    return { exitCode: 0, durationMs: 50, worktreeUnchanged: false };
  };
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  equal(h.controls.checkCalls.length, 1);
  const [first, second] = (await loadState(h.context)).verification!.checks;
  equal([first.status, second.status], ["failed", "failed"]);
  assert(!second.log && second.skipped?.includes("regression changed"));
  const { reason, presentation } = result(h);
  assert(
    reason.includes("regression (exit 0; changed the worktree, branch or HEAD"),
    reason,
  );
  assert(reason.includes("no longer clean at the reviewed commit"), reason);
  assert(!reason.includes("stale"), reason);
  // A skipped check did not itself change the worktree.
  assert(
    presentation.split("\n").some((line) =>
      line.startsWith("- [ ] caller:") &&
      !line.includes("; changed the worktree, branch or HEAD")
    ),
    presentation,
  );
});

Deno.test("a check the Git helper refuses is recorded as failed instead of discarding the run", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  h.controls.checkOutcome = () => {
    throw new Error("Worktree is not clean: /repo/issue");
  };
  await h.drive.execute({ agent: options }, h.context);
  const verification = (await loadState(h.context)).verification!;
  equal(verification.status, "failed");
  equal(
    verification.checks.map((check) => check.skipped),
    [
      "Could not run: Worktree is not clean: /repo/issue",
      "Not run: check regression could not run",
    ],
  );
  equal(result(h).decision, "blocked");
  assert(
    result(h).reason.includes(
      "regression (Could not run: Worktree is not clean: /repo/issue)",
    ),
    result(h).reason,
  );
});

Deno.test("a check stopped at its time limit fails as timed out and later checks still run", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  h.controls.checkOutcome = (request) =>
    (request.argv as string[]).includes("caller")
      ? { exitCode: 0, durationMs: 50, worktreeUnchanged: true }
      : {
        exitCode: null,
        durationMs: 3_600_000,
        worktreeUnchanged: true,
        timedOut: true,
      };
  await h.drive.execute({ agent: options }, h.context);
  equal(h.controls.checkCalls.length, 2);
  const verification = (await loadState(h.context)).verification!;
  equal(verification.status, "failed");
  equal(
    verification.checks.map((check) => [check.status, check.timedOut]),
    [["failed", true], ["passed", undefined]],
  );
  const { decision, reason, presentation } = result(h);
  equal(decision, "blocked");
  assert(reason.includes("regression (timed out; log:"), reason);
  assert(!reason.includes("caller ("), reason);
  assert(
    presentation.includes(
      "- [ ] regression: cargo x test -p demo -E 'test(/^fix::/)' -> timed out in 3600.0s",
    ),
    presentation,
  );
});

Deno.test("legacy plans keep the reviewer-reported gate without deterministic verification", async () => {
  for (const failed of [false, true]) {
    const state = implementing();
    delete state.plan!.checks;
    const h = harness(state);
    h.controls.evidenceForRole = (task) =>
      task.role === "review_code"
        ? {
          ...driverEvidence(task) as object,
          preliminaryChecks: [{
            command: "regression",
            result: failed ? "failed" : "passed",
            summary: "Recorded result",
          }],
        }
        : driverEvidence(task);
    await h.drive.execute({ agent: options }, h.context);
    equal(h.controls.checkCalls.length, 0);
    if (failed) {
      equal(result(h).decision, "blocked");
      assert(result(h).reason.includes("failed verification"));
    } else {
      equal(result(h).decision, "authorize_merge");
      assert(
        result(h).presentation.includes(
          "No deterministic verification: plan version 1 predates structured checks.",
        ),
      );
    }
  }
});

Deno.test("driver reconciles an interrupted result write and continues to review without repeating completed workers", async () => {
  const h = harness(transition(createState(issue), "triage", triage));
  h.controls.evidenceForRole = driverEvidence;
  h.controls.failTaskCompletionOnce = true;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "applying",
  );
  equal((await loadState(h.context)).plan?.version, 1);
  await h.drive.execute({ agent: { ...options, model: "changed" } }, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("original session/provider settings"));
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  equal(h.calls.length, 2);
  equal(h.calls.map((call) => call.method), ["run", "run"]);
  equal(result(h).planVersion, 1);
});

Deno.test("driver stops on stale requirements, invalid evidence and changed worktree commits", async () => {
  for (const scenario of ["requirements", "evidence", "commit"]) {
    const h = harness(
      scenario === "commit" ? implementing() : createState(issue),
    );
    h.controls.evidenceForRole = driverEvidence;
    if (scenario === "requirements") {
      h.controls.afterRun = () => {
        h.controls.agenda = { ...issue, digest: "e".repeat(64) };
      };
    }
    if (scenario === "evidence") {
      h.controls.evidenceForRole = () => "bad evidence";
    }
    if (scenario === "commit") h.controls.repo.head = "c".repeat(40);
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "blocked");
    equal(h.calls.length, 1);
    await h.drive.execute({}, h.context);
    equal(result(h).decision, "blocked");
    assert(h.calls.every((call) => call.name === h.calls[0].name));
  }
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  h.controls.agenda = { ...issue, digest: "e".repeat(64) };
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("requirements changed"));
  equal(h.calls.length, 3);
});

Deno.test("restart invalidates an old approval handoff and driver follows the replacement session without duplicates", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  const previous = result(h);
  await h.methods.restart_agent.execute({
    ...options,
    session: "replacement",
    control: { session: "replacement", paneId: "terminal_1" },
    role: "plan",
    taskId: previous.assignment!.id,
    request: "Restart planning in the replacement session",
  }, h.context);
  equal(result(h).status, "invalidated");
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_plan");
  assert(result(h).id !== previous.id);
  equal(result(h).planVersion, 2);
  equal(result(h).assignment?.config.session, "replacement");
  equal(h.calls.length, 5);
  await h.drive.execute({}, h.context);
  equal(h.calls.length, 5);
});

Deno.test("driver waits on the original acknowledged submission across timeouts and blocks dialogs, uncertain delivery and provider failures", async () => {
  for (
    const scenario of [
      "timeouts",
      "startup",
      "dialog",
      "uncertain",
      "failure",
      "transport-after-timeout",
    ]
  ) {
    const h = harness();
    const cli: string[][] = [];
    let waits = 0;
    h.controls.agentRunner = (args) => {
      cli.push(args);
      const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
      const agent = {
        pane_id: "terminal_7",
        session: options.session,
        agent: { kind: "claude", session_id: "conversation-1", state: "idle" },
        cwd: task.config.cwd,
        ready: true,
        status_available: true,
        prompt_supported: true,
      };
      let value: Record<string, unknown>;
      if (args[3] === "spawn") {
        value = {
          outcome: "ok",
          spawned: { pane_id: "terminal_7", cwd: task.config.cwd },
        };
      } else if (args.includes("--until")) {
        value = scenario === "startup" && cli.length === 2
          ? {
            outcome: "timed_out",
            agents: [{
              ...agent,
              agent: { ...agent.agent, state: "Busy" },
              not_ready_reason: "not_idle",
            }],
          }
          : { outcome: "ok", agents: [agent] };
      } else {
        const shouldTimeout = task.role === "triage" &&
          (args[3] === "prompt" || ++waits < 3);
        if (scenario === "transport-after-timeout" && args[3] === "wait") {
          return Promise.reject(new Error("Session disappeared"));
        }
        value = shouldTimeout
          ? {
            outcome: scenario === "failure" ? "failed" : "timed_out",
            submission_id: "submission-1",
            acknowledged: scenario !== "uncertain",
            agents: [{
              ...agent,
              agent: {
                ...agent.agent,
                state: scenario === "dialog" ? "Waiting" : "Busy",
              },
              not_ready_reason: "unresolved_turn",
            }],
          }
          : {
            outcome: "completed",
            submission_id: "submission-1",
            acknowledged: true,
            agents: [agent],
            message: JSON.stringify(driverEvidence(task)),
          };
      }
      return Promise.resolve({
        code: 0,
        stdout: JSON.stringify(value),
        stderr: "",
      });
    };
    await h.drive.execute({ agent: options }, h.context);
    if (["timeouts", "startup"].includes(scenario)) {
      equal(result(h).decision, "approve_plan");
      equal(cli.filter((args) => args[3] === "spawn").length, 3);
      equal(cli.filter((args) => args[3] === "prompt").length, 3);
      const observations = cli.filter((args) =>
        args[3] === "wait" && args.includes("--submission")
      );
      equal(observations.length, 3);
      for (const args of observations) {
        equal(args[args.indexOf("--submission") + 1], "submission-1");
        equal(args[args.indexOf("--conversation") + 1], "conversation-1");
      }
      await h.drive.execute({}, h.context);
      equal(cli.filter((args) => args[3] === "prompt").length, 3);
    } else {
      equal(result(h).decision, "blocked");
      equal(cli.filter((args) => args[3] === "spawn").length, 1);
      equal(cli.filter((args) => args[3] === "prompt").length, 1);
      const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
      await h.drive.execute({}, h.context);
      equal(result(h).decision, "blocked");
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).id,
        original.id,
      );
      equal(cli.filter((args) => args[3] === "prompt").length, 1);
    }
  }
});

Deno.test("driver reuses implementation after explicitly discarding unusable review evidence", async () => {
  const h = harness(implementing());
  h.controls.evidenceForRole = (task) =>
    task.role === "review_code" ? "bad review evidence" : driverEvidence(task);
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  await h.methods.discard_agent.execute({
    request: "Discard the malformed completed review",
    confirmedStopped: true,
  }, h.context);
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_merge");
  equal(dispatchedRoles(h), ["implement", "review_code", "review_code"]);
  assert(result(h).presentation.includes("Repaired the affected caller"));
  h.controls.repo.head = "c".repeat(40);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("review evidence is stale"));
  equal(h.calls.length, 3);
});

Deno.test("driver survives controller interruption and resumes its saved assignment with no second submission", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  const abort = new AbortController();
  h.context.signal = abort.signal;
  const run = h.context.runModel;
  h.context.runModel = () => {
    abort.abort(new Error("Controller interrupted before child launch"));
    return Promise.reject(abort.signal.reason);
  };
  await rejects(
    () => h.drive.execute({ agent: options }, h.context),
    "Controller interrupted",
  );
  const saved = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(saved.role, "triage");
  equal(saved.status, "running");
  equal(h.calls.length, 0);
  h.context.signal = new AbortController().signal;
  h.context.runModel = run;
  await h.drive.execute({ agent: { ...options, model: "changed" } }, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("original session/provider settings"));
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  equal(h.calls[0].name, saved.behaviorName);
  equal(h.calls[0].method, "recover");
  equal(h.calls.length, 3);
});

Deno.test("driver blocks a pending rollback and missing configuration without dispatch", async () => {
  const h = harness();
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("Select a running session"));
  h.controls.reopenError = "Agenda unavailable";
  await rejects(
    () =>
      h.methods.rollback.execute({
        step: "triage",
        request: "Restart investigation",
      }, h.context),
    "Agenda unavailable",
  );
  equal(result(h).status, "invalidated");
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("Rollback is incomplete"));
  equal(h.calls.length, 0);
});

Deno.test("driver honors a completed review-only rollback without repeating implementation", async () => {
  const h = harness(implementing());
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  await h.methods.rollback.execute({
    step: "review_code",
    request: "Repeat code review",
  }, h.context);
  await h.drive.execute({}, h.context);
  equal(dispatchedRoles(h), ["implement", "review_code", "review_code"]);
  equal(result(h).decision, "authorize_merge");
  equal(result(h).commit, head);
});

Deno.test("driver ignores unrelated or superseded review rollback records", async () => {
  for (
    const patch of [
      { id: "00000000-0000-4000-8000-999999999999" },
      { status: "superseded" },
    ]
  ) {
    const h = harness(implementing());
    h.controls.evidenceForRole = driverEvidence;
    await h.drive.execute({ agent: options }, h.context);
    await h.methods.rollback.execute({
      step: "review_code",
      request: "Repeat code review",
    }, h.context);
    h.resources.set("recovery", {
      ...h.resources.get("recovery") as object,
      ...patch,
    });
    await h.drive.execute({}, h.context);
    equal(dispatchedRoles(h), [
      "implement",
      "review_code",
      "implement",
      "review_code",
    ]);
    equal(result(h).decision, "authorize_merge");
  }
});

Deno.test("driver omits obsolete implementation summaries for replacement reviewed commits and cached handoffs", async () => {
  const h = harness(implementing());
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  const original = result(h);
  assert(original.implementation);
  const replacement = "c".repeat(40);
  h.controls.repo.head = replacement;
  h.controls.evidenceForRole = (task) => ({
    ...driverEvidence(task) as object,
    commit: replacement,
    ...(task.role === "implement"
      ? { summary: "Replacement implementation" }
      : {}),
  });
  await h.methods.run_agent.execute(
    { ...options, role: "implement" },
    h.context,
  );
  await h.methods.run_agent.execute(
    { ...options, role: "review_code" },
    h.context,
  );
  await h.drive.execute({}, h.context);
  const fresh = result(h);
  equal(fresh.decision, "authorize_merge");
  equal(fresh.commit, replacement);
  equal(fresh.implementation, undefined);
  assert(!fresh.presentation.includes("Repaired the affected caller"));
  await h.drive.execute({}, h.context);
  equal(result(h).id, fresh.id);

  // Simulate a handoff cached by the old driver for the same reviewed state.
  h.resources.set("handoff", {
    ...fresh,
    implementation: original.implementation,
    presentation: original.presentation,
  });
  await h.drive.execute({}, h.context);
  assert(result(h).id !== fresh.id);
  equal(result(h).implementation, undefined);
  assert(!result(h).presentation.includes("Repaired the affected caller"));
});

// Exercise the real adapter under the lifecycle, including new tasks created by
// revisions and recovery. Moving the control pane after every prompt ensures a
// cached tab cannot satisfy the next spawn.
Deno.test("every lifecycle role, plan revision, repair and restart resolves the control pane anew", async () => {
  const h = harness();
  const launched: { role: string; tab: string }[] = [];
  let firstReview = true;
  h.controls.agentRunner = (args) => {
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    let value: Record<string, unknown>;
    if (args[3] === "spawn") {
      equal(task.config.control, options.control);
      equal(args[args.indexOf("--stack-for") + 1], options.control.paneId);
      const tab = String(h.controls.controlTab);
      launched.push({ role: task.role, tab });
      value = {
        outcome: "ok",
        spawned: { pane_id: "terminal_7", cwd: task.config.cwd },
      };
    } else {
      const prompt = args[3] === "prompt";
      let evidence = driverEvidence(task);
      if (prompt && task.role === "review_plan" && firstReview) {
        firstReview = false;
        evidence = {
          planVersion: 1,
          findings: [{
            id: "coverage",
            severity: "high",
            description: "Add missing coverage",
            requirement: "Caller coverage",
            mechanism: "Caller can bypass validation",
            evidence: ["src/lib.rs:42"],
          }],
        };
      }
      if (prompt) h.controls.controlTab++;
      value = {
        outcome: prompt ? "completed" : "ok",
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
        submission_id: prompt ? "submission-1" : undefined,
        message: prompt ? JSON.stringify(evidence) : undefined,
      };
    }
    return Promise.resolve({
      code: 0,
      stdout: JSON.stringify(value),
      stderr: "",
    });
  };
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  equal(launched.map((launch) => launch.role), [
    "triage",
    "plan",
    "review_plan",
    "plan",
    "review_plan",
  ]);
  let state = transition(await loadState(h.context), "approve", {
    planVersion: 2,
    request: "Implement",
  });
  state = transition(state, "implement", {
    worktree: "/repo/issue",
    branch: "issue-85",
  });
  h.resources.set("state", state);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_merge");
  // Explicitly roll back for a repair, then use the same foreground driver.
  await h.methods.rollback.execute({
    step: "implement",
    request: "Repair implementation",
  }, h.context);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_merge");
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const restart = {
    ...options,
    role: "review_code" as const,
    taskId: task.id,
    request: "Restart code review",
  };
  await h.methods.restart_agent.execute(restart, h.context);
  const count = launched.length;
  await h.methods.restart_agent.execute(restart, h.context);
  await h.drive.execute({}, h.context);
  equal(launched.length, count);
  equal(launched.map((launch) => launch.role), [
    "triage",
    "plan",
    "review_plan",
    "plan",
    "review_plan",
    "implement",
    "review_code",
    "implement",
    "review_code",
    "review_code",
  ]);
  equal(
    launched.map((launch) => launch.tab),
    Array.from({ length: count }, (_, index) => String(index)),
  );
  equal(h.controls.placementCalls.length, count);
});

for (const legacy of [false, true]) {
  for (const explicit of [false, true]) {
    Deno.test(`driver retains updated control after adopting a completed ${legacy ? "legacy" : "standalone"} task with ${explicit ? "explicit" : "implicit"} settings`, async () => {
      const h = harness();
      h.controls.evidenceForRole = driverEvidence;
      await h.drive.execute({ agent: options }, h.context);
      equal(result(h).decision, "approve_plan");
      const previous = DriverSchema.parse(h.resources.get("driver"));
      await h.methods.run_agent.execute(
        { ...options, role: "review_plan" },
        h.context,
      );
      const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
      equal(task.status, "completed");
      assert(task.id !== previous.assignmentId);
      if (legacy) {
        delete task.config.control;
        h.resources.set("agent-task", task);
      }

      const control = { ...options.control, paneId: "terminal_2" };
      await h.drive.execute({
        ...(explicit ? { agent: options } : {}),
        control,
      }, h.context);
      equal(result(h).decision, "approve_plan");
      // Direct driver invocations provide no environment-derived control input.
      await h.drive.execute({}, h.context);
      equal(result(h).decision, "approve_plan");
      const saved = DriverSchema.parse(h.resources.get("driver"));
      equal(saved.agent?.control, control);
      equal(saved.assignmentId, task.id);
      equal(h.resources.get("agent-task"), task);

      let state = transition(await loadState(h.context), "approve", {
        planVersion: 1,
        request: "Implement",
      });
      state = transition(state, "implement", {
        worktree: "/repo/issue",
        branch: "issue-85",
      });
      h.resources.set("state", state);
      const count = h.calls.length;
      await h.drive.execute({}, h.context);
      equal(result(h).decision, "authorize_merge");
      equal(h.calls.slice(count).map((call) => call.arguments?.control), [
        control,
        control,
      ]);
    });
  }
}

Deno.test("legacy driver settings require control identity for new workers", async () => {
  const h = harness();
  const { control: _control, ...legacy } = options;
  h.resources.set("driver", { agent: legacy });
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("require control identity"));
  equal(h.calls.length, 0);
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ control: options.control }, h.context);
  equal(result(h).decision, "approve_plan");
  assert(
    h.calls.every((call) =>
      (call.arguments as { control: unknown }).control != null
    ),
  );
});

Deno.test("legacy existing submissions resume unchanged while future workers acquire control identity", async () => {
  const h = harness();
  h.controls.runError = "Interrupted observation";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Interrupted observation",
  );
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  delete task.config.control;
  h.resources.set("agent-task", task);
  const child = AgentStateSchema.parse(h.child.get("state"));
  delete child.config.control;
  h.child.set("state", child);
  h.controls.runError = undefined;
  h.controls.evidenceForRole = driverEvidence;
  const control = { ...options.control, paneId: "terminal_2" };
  await h.drive.execute({ control }, h.context);
  equal(result(h).decision, "approve_plan");
  equal(h.calls[1].method, "recover");
  equal(h.calls[1].arguments?.control, undefined);
  equal(h.calls.slice(2).map((call) => call.arguments?.control), [
    control,
    control,
  ]);
});

Deno.test("lifecycle refuses mismatched control sessions before creating a worker", async () => {
  const h = harness();
  await rejects(
    () =>
      h.methods.run_agent.execute({
        ...options,
        role: "triage",
        control: { ...options.control, session: "other" },
      }, h.context),
    "different session",
  );
  equal(h.calls.length, 0);
  assert(!h.resources.has("agent-task"));
});

Deno.test("legacy unlaunched workers cannot recover through automatic placement", async () => {
  for (const missing of [false, true]) {
    const h = harness();
    h.controls.childPhase = "preflight";
    h.controls.runError = "Interrupted launch";
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
      "Interrupted launch",
    );
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    delete task.config.control;
    h.resources.set("agent-task", task);
    if (missing) h.child.clear();
    else {
      const child = AgentStateSchema.parse(h.child.get("state"));
      delete child.config.control;
      h.child.set("state", child);
    }
    await rejects(
      () => h.methods.resume_agent.execute({}, h.context),
      missing
        ? "no durable agent state"
        : "requires a restart with control identity",
    );
    equal(h.calls.length, 1);
  }
});

Deno.test("accepted evidence survives failed cleanup and resume closes without rerunning", async () => {
  const h = harness();
  h.controls.cleanupError = true;
  await rejects(() =>
    h.methods.run_agent.execute(
      h.methods.run_agent.arguments.parse({ ...options, role: "triage" }),
      h.context,
    ), "cleanup blocked");
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(task.status, "completed");
  equal(task.cleanup, "pending");
  const state = structuredClone(h.resources.get("state"));
  h.controls.cleanupError = false;
  await h.methods.resume_agent.execute({}, h.context);
  equal(h.calls.length, 1);
  equal(h.controls.cleanupCalls, 2);
  equal(h.resources.get("state"), state);
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).cleanup, "done");
});

Deno.test("completion returns only the latest task handle, including applying resume", async () => {
  for (const interrupted of [false, true]) {
    const h = harness();
    h.controls.failTaskCompletionOnce = interrupted;
    const run = () =>
      h.methods.run_agent.execute(
        h.methods.run_agent.arguments.parse({ ...options, role: "triage" }),
        h.context,
      );
    let output;
    if (interrupted) {
      await rejects(run, "task completion write failed");
      output = await h.methods.resume_agent.execute({}, h.context);
    } else output = await run();
    const names = output.dataHandles.map((handle) => handle.name);
    equal(new Set(names).size, names.length);
    equal(names.filter((name) => name === "agent-task").length, 1);
    equal(AgentTaskSchema.parse(h.resources.get("agent-task")).cleanup, "done");
  }
});

Deno.test("rollback clears failed cleanup and legacy superseded cleanup does not block", async () => {
  const h = harness();
  h.controls.cleanupError = true;
  await rejects(() =>
    h.methods.run_agent.execute(
      h.methods.run_agent.arguments.parse({ ...options, role: "triage" }),
      h.context,
    ), "cleanup blocked");
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  await h.methods.rollback.execute(
    { step: "triage", request: "Start again" },
    h.context,
  );
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).cleanup,
    undefined,
  );
  equal(
    AgentTaskSchema.parse(h.resources.get(`agent-task-${task.id}`)).cleanup,
    undefined,
  );
  h.resources.set("agent-task", {
    ...h.resources.get("agent-task") as object,
    cleanup: "pending",
  });
  await assertNoPendingAgent(h.context);
  h.controls.cleanupError = false;
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
});

function blockedInitialPlan() {
  let state = transition(createState(issue), "triage", triage);
  state = transition(state, "plan", driverPlan);
  return transition(state, "review_plan", {
    planVersion: 1,
    findings: [{
      id: "gap",
      severity: "critical",
      description: "Cover the caller",
      requirement: "Caller coverage",
      mechanism: "Caller bypasses validation",
      evidence: ["src/lib.rs:42"],
    }],
  });
}

for (
  const fault of ["reservation", "dispatch", "worker", "event", "completion"]
) {
  Deno.test(`automatic revision survives interrupted ${fault} without replenishing its allowance`, async () => {
    const h = harness(blockedInitialPlan());
    h.controls.evidenceForRole = blockedPlanEvidence;
    if (fault === "reservation") {
      h.controls.failWrite = (spec, _name, value) =>
        spec === "driver" && !!value.automaticRevision;
    }
    if (fault === "dispatch") {
      h.controls.failWrite = (spec) => spec === "agent-task";
    }
    if (fault === "worker") h.controls.runError = "Interrupted observation";
    if (fault === "event") h.controls.failEventOnce = true;
    if (fault === "completion") h.controls.failTaskCompletionOnce = true;
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "blocked");
    const reserved =
      DriverSchema.parse(h.resources.get("driver")).automaticRevision;
    equal(!!reserved, fault !== "reservation");
    if (fault === "dispatch") {
      await rejects(
        () => assertNoPendingAgent(h.context),
        "Reserved pane assignment",
      );
    }
    h.controls.runError = undefined;
    await h.drive.execute({}, h.context);
    equal(result(h).decision, "authorize_plan_revision");
    equal(result(h).planVersion, 2);
    const automatic = DriverSchema.parse(h.resources.get("driver"))
      .automaticRevision!;
    if (reserved) equal(automatic, reserved);
    equal(
      h.calls.filter((c) =>
        c.name === automatic.assignment.behaviorName && c.method === "run"
      ).length,
      1,
    );
    const count = h.calls.length;
    for (let retry = 0; retry < 3; retry++) {
      await h.drive.execute({}, h.context);
    }
    equal(h.calls.length, count);
    await h.methods.rollback.execute({
      step: "plan",
      request: "Reconsider scope",
    }, h.context);
    await h.drive.execute({}, h.context);
    equal(result(h).decision, "authorize_plan_revision");
    equal(h.calls.length, count);
    equal(
      DriverSchema.parse(h.resources.get("driver")).automaticRevision,
      automatic,
    );
  });
}

Deno.test("adjudication advances the driver without another submission or implementation approval", async () => {
  const h = harness();
  h.controls.evidenceForRole = blockedPlanEvidence;
  await h.drive.execute({ agent: options }, h.context);
  const handoff = result(h);
  const original = await loadState(h.context);
  const adjudicated = transition(original, "adjudicate_plan", {
    planVersion: 2,
    reviewFingerprint: handoff.reviewFingerprint,
    dispositions: [{
      findingId: "gap",
      disposition: "rejected",
      rationale: "Inspected caller; existing guard covers it",
      evidence: ["src/lib.rs:42"],
    }],
  });
  await saveState(h.context, "adjudicate_plan", adjudicated);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_plan");
  equal(h.calls.length, 5);
  equal(result(h).state.planReview, original.planReview);
  assert(effectivePlanFindings(result(h).state)[0].resolution);
  equal(result(h).state.approval, undefined);
  assert(result(h).presentation.includes("Inspected caller"));
});

Deno.test("revision and restart deliver exact feedback and cumulative context", async () => {
  const h = harness();
  h.controls.evidenceForRole = blockedPlanEvidence;
  await h.drive.execute({ agent: options }, h.context);
  const request = "  Preserve the public API.\nAddress only the caller gap.  ";
  await h.drive.execute({
    revisionApproval: { handoffId: result(h).id, request },
  }, h.context);
  const authorization = DriverSchema.parse(h.resources.get("driver"))
    .revisionAuthorization!;
  assert(authorization.assignment.prompt.includes(request));
  equal(authorization.request, request);
  const current = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const restartRequest =
    "Keep the selected tests; explain the disputed finding.";
  await h.methods.restart_agent.execute({
    ...options,
    role: "plan",
    taskId: current.id,
    request: restartRequest,
  }, h.context);
  const replacement = AgentTaskSchema.parse(h.resources.get("agent-task"));
  assert(replacement.prompt.includes(restartRequest));
  const state = await loadState(h.context);
  assert(
    state.planningContext!.userFeedback.some((f) => f.request === request),
  );
  assert(
    state.planningContext!.userFeedback.some((f) =>
      f.request === restartRequest
    ),
  );
  assert(
    state.planningContext!.acceptedDecisions.includes(
      "Keep the existing caller API",
    ),
  );
  assert(
    state.planningContext!.history.some((h) =>
      h.plan.version === 2 && h.review?.findings[0].id === "gap"
    ),
  );
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_plan_revision");
  equal(state.planVersion, 4);
});

Deno.test("new revision contracts reject missing responses while old assignments import unchanged", async () => {
  for (const legacy of [false, true]) {
    const h = harness(blockedInitialPlan(), driverPlan);
    h.controls.runError = "Interrupted observation";
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "plan" }, h.context),
      "Interrupted observation",
    );
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    const prompt = task.prompt;
    if (legacy) delete task.contractVersion;
    h.resources.set("agent-task", task);
    h.controls.runError = undefined;
    h.controls.evidence = driverPlan;
    if (legacy) {
      await h.methods.resume_agent.execute({}, h.context);
      equal((await loadState(h.context)).planVersion, 2);
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).prompt,
        prompt,
      );
    } else {
      await rejects(
        () => h.methods.resume_agent.execute({}, h.context),
        "revision",
      );
      equal((await loadState(h.context)).planVersion, 1);
    }
  }
  const h = harness(blockedInitialPlan());
  h.controls.evidenceForRole = (task) => ({
    ...driverEvidence(task) as object,
    revision: {
      summary: "Changed tests",
      responses: [],
      acceptedDecisions: [],
    },
  });
  await rejects(
    () => h.methods.run_agent.execute({ ...options, role: "plan" }, h.context),
    "respond to blocking finding gap",
  );
});

Deno.test("legacy first plans get one automatic revision and later versions get none", async () => {
  for (const version of [1, 2]) {
    const state = blockedInitialPlan();
    delete state.planningContext;
    state.planVersion = version;
    state.plan!.version = version;
    state.planReview!.planVersion = version;
    const h = harness(state);
    h.controls.evidenceForRole = blockedPlanEvidence;
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "authorize_plan_revision");
    equal(h.calls.length, version === 1 ? 2 : 0);
    equal(result(h).planVersion, 2);
  }
});

Deno.test("a saved legacy revision authorization resumes its original contract and prompt", async () => {
  const h = harness();
  h.controls.evidenceForRole = blockedPlanEvidence;
  await h.drive.execute({ agent: options }, h.context);
  h.controls.failWrite = (spec) => spec === "agent-task";
  await h.drive.execute({ revisionApproval: revisionApproval(h) }, h.context);
  const driver = DriverSchema.parse(h.resources.get("driver"));
  const task = driver.revisionAuthorization!.assignment;
  delete task.contractVersion;
  delete task.request;
  task.prompt = "Original legacy planner prompt; return the old plan contract";
  task.promptDigest = await promptDigest(task.prompt);
  h.resources.set("driver", driver);
  h.controls.evidenceForRole = (task) =>
    task.role === "plan" ? driverPlan : blockedPlanEvidence(task);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_plan_revision");
  equal(result(h).planVersion, 3);
  equal(h.calls.filter((call) => call.name === task.behaviorName).length, 1);
  equal(
    h.calls.find((call) => call.name === task.behaviorName)?.arguments?.prompt,
    task.prompt,
  );
  equal(
    DriverSchema.parse(h.resources.get("driver")).revisionAuthorization!
      .assignment.prompt,
    task.prompt,
  );
});

Deno.test("restart preserves feedback from a reserved revision that never dispatched", async () => {
  const h = harness();
  h.controls.evidenceForRole = blockedPlanEvidence;
  await h.drive.execute({ agent: options }, h.context);
  const approval = {
    handoffId: result(h).id,
    request: "Keep the caller API and selected tests",
  };
  h.controls.failWrite = (spec) => spec === "agent-task";
  await h.drive.execute({ revisionApproval: approval }, h.context);
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  await h.methods.restart_agent.execute({
    ...options,
    role: "plan",
    taskId: task.id,
    request: "Restart this targeted revision in the new session",
  }, h.context);
  const replacement = AgentTaskSchema.parse(h.resources.get("agent-task"));
  assert(replacement.prompt.includes(approval.request));
  assert(replacement.prompt.includes("Restart this targeted revision"));
  equal(
    replacement.before.planningContext!.userFeedback.filter((f) =>
      f.request === approval.request
    ).length,
    1,
  );
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_plan_revision");
  equal(result(h).planVersion, 3);
});

Deno.test("origin persists across all worker roles, later drive and explicit restart", async () => {
  const origin = "/repo/issue";
  const h = harness(createState(issue, origin));
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  equal(dispatchedRoles(h), ["triage", "plan", "review_plan"]);
  let state = await loadState(h.context);
  equal(state.workerWorktree, origin);
  state = transition(state, "approve", {
    planVersion: 1,
    request: "Implement",
  });
  state = transition(state, "implement", {
    worktree: origin,
    branch: "issue-85",
  });
  h.resources.set("state", state);
  h.context.repoDir = "/repo/later-invocation";
  await h.drive.execute({}, h.context);
  equal(dispatchedRoles(h), [
    "triage",
    "plan",
    "review_plan",
    "implement",
    "review_code",
  ]);
  const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const restart = {
    ...options,
    role: "plan" as const,
    taskId: old.id,
    request: "Restart planning",
  };
  await h.methods.restart_agent.execute(restart, h.context);
  await h.methods.restart_agent.execute(restart, h.context);
  await h.drive.execute({}, h.context);
  // Recovery persists its replacement before runTask saves it again; count
  // actual child calls rather than the duplicate running-task writes.
  equal(h.calls.length, 7);
  assert(h.calls.every((call) => call.arguments?.cwd === origin));
  equal((await loadState(h.context)).workerWorktree, origin);
  equal((await loadState(h.context)).planVersion, 2);
});

Deno.test("legacy workers retain implementation/control selection and pending tasks keep saved config", async () => {
  for (const initial of [createState(issue), implementing()]) {
    const h = harness(initial);
    h.controls.evidenceForRole = driverEvidence;
    h.controls.runError = "Interrupted submission";
    const role = initial.implementation ? "implement" : "triage";
    const expected = initial.implementation?.worktree ?? h.context.repoDir;
    await rejects(
      () => h.methods.run_agent.execute({ ...options, role }, h.context),
      "Interrupted submission",
    );
    const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(original.config.cwd, expected);
    h.context.repoDir = "/repo/later-invocation";
    h.controls.runError = undefined;
    await h.methods.resume_agent.execute({}, h.context);
    equal(h.calls.map((call) => call.arguments?.cwd), [expected, expected]);
    equal(
      AgentTaskSchema.parse(h.resources.get("agent-task")).config,
      original.config,
    );
    equal((await loadState(h.context)).workerWorktree, undefined);
  }
});
