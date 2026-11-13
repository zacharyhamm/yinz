import { launchReply, launchSupport } from "./launch_fixtures.ts";
import {
  createEvidenceMethods,
  type EvidenceContext,
} from "../issue_evidence.ts";
import { SubmissionSchema, SubmitArgsSchema } from "../_lib/issue_evidence.ts";
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
  codeReviewFingerprint,
  createState,
  effectivePlanFindings,
  initialPlanning,
  type LifecycleState,
  priorImplementationFindings,
  recordMergeFailure,
  refreshIssue,
  selectedRepairFindings,
  transition,
} from "../_lib/lifecycle.ts";
import { GitOperationError, loadState, saveState } from "../_lib/runtime.ts";
import {
  AgentConfigSchema,
  type AgentContext,
  AgentStateSchema,
  confirmFailureAgent,
  continueAgent,
  promptDigest,
  recoverAgent,
  runAgent,
  type YinzRunner,
} from "../_lib/yinz_agent.ts";

function withUx(state: LifecycleState): LifecycleState {
  return transition(state, "review_ux", {
    planVersion: state.plan!.version,
    commit: state.codeReview!.commit,
    assessment: {
      impact: "no_material_impact",
      summary: "Fixture caller behavior and command output remain unchanged.",
      assessedPaths: ["src/lib.rs: affected caller and repeat command use"],
      limitations: ["Source-based fixture assessment"],
    },
    findings: [],
  });
}

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
  state = withUx(state);
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

function harness(
  initial = createState(issue),
  evidence: unknown = triage,
  offerCloseout?: IssueAgentDependencies["offerCloseout"],
) {
  const resources = new Map<string, unknown>([["state", initial]]);
  const child = new Map<string, Record<string, unknown>>();
  const submissions = new Map<string, Map<string, unknown>>();
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
    failEvidenceOpen: false,
    failEvidenceSupersede: false,
    submitEvidence: true,
    cleanupCalls: 0,
    cleanupError: false,
    evidence,
    evidenceForRole: undefined as
      | ((task: ReturnType<typeof AgentTaskSchema.parse>) => unknown)
      | undefined,
    agenda: issue,
    failEventOnce: false,
    failTaskCompletionOnce: false,
    failChildResultOnce: false,
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
    agentId: () => "submission-1",
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
        let value: unknown;
        if (type === "@yinz/issue-evidence") {
          value = submissions.get(name)?.get(resource);
        } else if (type === "@yinz/issue-lifecycle") {
          equal(name, context.modelId);
          value = resources.get(resource);
        } else {
          equal(type, "@yinz/agent");
          equal(
            name,
            AgentTaskSchema.parse(resources.get("agent-task")).behaviorName,
          );
          value = name === childBehavior ? child.get(resource) : undefined;
        }
        return Promise.resolve(
          value == null
            ? null
            : new TextEncoder().encode(JSON.stringify(value)),
        );
      },
    },
    runModel: async (request) => {
      if (request.modelType === "@yinz/issue-evidence") {
        if (request.method === "open" && controls.failEvidenceOpen) {
          throw new Error("Evidence open interrupted");
        }
        if (request.method === "supersede" && controls.failEvidenceSupersede) {
          throw new Error("Evidence supersession interrupted");
        }
        const evidenceContext = submissionContext(request.name!);
        const methods = evidenceMethods();
        if (request.method === "open") {
          await methods.open.execute(
            methods.open.arguments.parse(request.arguments),
            evidenceContext,
          );
        } else if (request.method === "supersede") {
          await methods.supersede.execute(
            methods.supersede.arguments.parse(request.arguments),
            evidenceContext,
          );
        } else throw new Error(`Unexpected evidence method: ${request.method}`);
        return { ok: true };
      }
      if (request.method === "cleanup") {
        const task = AgentTaskSchema.parse(resources.get("agent-task"));
        equal(task.status, "completed");
        equal(task.cleanup, "pending");
        controls.cleanupCalls++;
        return controls.cleanupError
          ? {
            ok: false,
            error: {
              message:
                "Worker cleanup blocked: rejected: worker background work is still running",
            },
          }
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
            if (name === "result" && controls.failChildResultOnce) {
              controls.failChildResultOnce = false;
              return Promise.reject(new Error("child publication interrupted"));
            }
            child.set(name, structuredClone(value));
            return Promise.resolve({ name });
          },
        };
        const dependencies = {
          publishPromptArtifact: () => {
            throw new Error(
              "Unexpected artifact publication in lifecycle fixture",
            );
          },
          run: async (args: string[], stdin?: string, signal?: AbortSignal) => {
            if (args[3] === "launch-support") {
              return {
                code: 0,
                stdout: JSON.stringify(launchSupport),
                stderr: "",
              };
            }
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
            const result = await controls.agentRunner!(args, stdin, signal);
            try {
              result.stdout = JSON.stringify(
                launchReply(args, JSON.parse(result.stdout)),
              );
            } catch { /* Preserve malformed responses. */ }
            if (args[3] === "prompt" && controls.submitEvidence) {
              await submitEvidence();
            }
            return result;
          },
          realPath: (path: string) => Promise.resolve(path),
          id: controls.agentId,
          now: () => timestamp,
        };
        try {
          if (request.method === "run") {
            child.clear();
            await runAgent({ prompt: task.prompt }, agentContext, dependencies);
          } else if (request.method === "continue") {
            await continueAgent(
              request.arguments as { id: string; prompt: string },
              agentContext,
              dependencies,
            );
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
      if (
        controls.submitEvidence &&
        ["run", "recover", "continue"].includes(request.method)
      ) await submitEvidence();
      controls.afterRun();
      return controls.runError
        ? { ok: false, error: { message: controls.runError } }
        : { ok: true };
    },
  };
  const dependencies: IssueAgentDependencies = {
    offerCloseout,
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
      if (request.action === "review-context") {
        return Promise.resolve({
          main: controls.repo.main,
          mergeBase: controls.repo.main,
          head: controls.repo.head,
          featureDiff: "fixture full feature diff",
          ancestry: "initial",
          baselineChanged: false,
          limitations: [],
        });
      }
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
    submissionPreflight: () => Promise.resolve(),
  };
  function submissionContext(name: string): EvidenceContext {
    let records = submissions.get(name);
    if (!records) {
      records = new Map();
      submissions.set(name, records);
    }
    const saved = records;
    const task = AgentTaskSchema.parse(resources.get("agent-task"));
    return {
      ...context,
      repoDir: task.submission?.controlDir ?? context.repoDir,
      definition: { name },
      readResource: (key) =>
        Promise.resolve(structuredClone(saved.get(key) ?? null)),
      writeResource: (_spec, key, value) => {
        saved.set(key, structuredClone(value));
        return Promise.resolve({ name: key });
      },
    };
  }
  function evidenceMethods() {
    return createEvidenceMethods({
      readAgenda: dependencies.readAgenda,
      inspect: () => dependencies.inspect(context),
      realPath: dependencies.realPath,
      now: dependencies.now,
    });
  }
  async function submitEvidence() {
    const task = AgentTaskSchema.parse(resources.get("agent-task"));
    if (!task.submission) return;
    const input = SubmitArgsSchema.parse({
      taskId: task.id,
      assignmentDigest: task.submission.assignmentDigest,
      role: task.role,
      evidence: controls.evidenceForRole?.(task) ?? controls.evidence,
    });
    await evidenceMethods().submit.execute(
      input,
      submissionContext(task.submission.behaviorName),
    );
  }
  return {
    context,
    controls,
    resources,
    writes,
    calls,
    child,
    submissions,
    submitEvidence,
    methods: createAgentMethods(dependencies),
    drive: createDriveMethod(dependencies),
  };
}

/** Exercise the real provider state machine while controlling turn responses. */
function observedHarness(
  initial = createState(issue),
  evidence: unknown = triage,
) {
  const h = harness(initial, evidence);
  const cli: { args: string[]; stdin?: string }[] = [];
  const turns: {
    response: Record<string, unknown> | Error;
    submit?: boolean;
  }[] = [];
  let serial = 0;
  h.controls.agentId = () => `submission-${++serial}`;
  h.controls.submitEvidence = false;
  h.controls.agentRunner = async (args, stdin) => {
    cli.push({ args, stdin });
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
      value = { outcome: "ok", agents: [agent] };
    } else {
      const turn = turns.shift();
      assert(turn, `Unexpected turn command: ${args.join(" ")}`);
      if (turn.submit) await h.submitEvidence();
      if (turn.response instanceof Error) throw turn.response;
      value = {
        agents: [agent],
        submission_id: args[args.indexOf("--submission") + 1],
        ...turn.response,
      };
    }
    return { code: 0, stdout: JSON.stringify(value), stderr: "" };
  };
  return { ...h, cli, turns };
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

Deno.test("issue agent ignores presentation differences after evidence acceptance", async () => {
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

Deno.test("truncated or arbitrary final chat cannot invalidate accepted evidence or repeat a review", async () => {
  for (const message of ['{"findings":[', "Anything at all", "", undefined]) {
    const h = harness();
    h.controls.evidenceForRole = driverEvidence;
    h.controls.resultPatch = { message };
    h.controls.childPatch = { message: "Different presentation" };
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "approve_plan");
    const receipt =
      AgentTaskSchema.parse(h.resources.get("agent-task")).evidence;
    assert(receipt?.payload.role === "review_plan");
    const calls = h.calls.length;
    await h.drive.execute({}, h.context);
    equal(h.calls.length, calls);
    equal(
      AgentTaskSchema.parse(h.resources.get("agent-task")).evidence,
      receipt,
    );
  }
});

Deno.test("provider completion without accepted evidence stays unresolved even with valid JSON chat", async () => {
  const h = harness();
  h.controls.submitEvidence = false;
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "submission_required",
  );
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).status, "running");
  equal((await loadState(h.context)).phase, "triaging");
  await rejects(
    () => h.methods.resume_agent.execute({}, h.context),
    "submission_required",
  );
  await rejects(() => assertNoPendingAgent(h.context), "unresolved");
  equal(h.calls.filter((c) => c.method === "run").length, 1);
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  h.controls.submitEvidence = true;
  await h.methods.continue_agent.execute({
    taskId: task.id,
    request: "Submit the existing findings",
  }, h.context);
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).id, task.id);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "completed",
  );
  equal(h.calls.map((c) => c.method), ["run", "recover", "continue"]);
});

for (const recovery of ["resume", "drive"] as const) {
  Deno.test(`a different continuation request preserves the timed-out turn for ${recovery}`, async () => {
    const h = observedHarness();
    h.controls.evidenceForRole = driverEvidence;
    h.turns.push({ response: { outcome: "completed" } });
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
      "submission_required",
    );
    const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
    const args = {
      taskId: original.id,
      request: "Submit the existing findings",
    };
    h.turns.push({ response: { outcome: "timed_out", acknowledged: true } });
    await rejects(
      () => h.methods.continue_agent.execute(args, h.context),
      "timed_out",
    );
    const pending = AgentTaskSchema.parse(h.resources.get("agent-task"));
    assert(pending.continuation);
    const child = AgentStateSchema.parse(h.child.get("state"));
    equal(child.phase, "waiting");
    // Launch intent reserves the first ID before the initial submission.
    equal(child.submissionId, "submission-3");
    equal(child.continuation?.id, pending.continuation.id);
    const calls = h.calls.length;
    await rejects(
      () =>
        h.methods.continue_agent.execute({
          ...args,
          request: "Different request text",
        }, h.context),
      "original completed, retained conversation",
    );
    equal(
      AgentTaskSchema.parse(h.resources.get("agent-task")).continuation,
      pending.continuation,
    );
    equal(h.child.get("state"), child);
    equal(h.calls.length, calls);
    h.turns.push({ response: { outcome: "completed" }, submit: true });
    if (recovery === "drive") {
      h.turns.push(
        { response: { outcome: "completed" }, submit: true },
        { response: { outcome: "completed" }, submit: true },
      );
      await h.drive.execute({ agent: options }, h.context);
      equal(result(h).decision, "approve_plan");
    } else {
      await h.methods.resume_agent.execute({}, h.context);
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).status,
        "completed",
      );
    }
    equal(h.calls[calls].method, "continue");
    equal(h.calls[calls].arguments, {
      id: pending.continuation.id,
      prompt: pending.continuation.prompt,
    });
    const observation = h.cli.find((c) =>
      c.args[3] === "wait" && c.args.includes("--submission")
    );
    assert(observation);
    equal(
      observation.args[observation.args.indexOf("--submission") + 1],
      child.submissionId,
    );
    equal(
      observation.args[observation.args.indexOf("--conversation") + 1],
      child.conversationId,
    );
    equal(h.cli.filter((c) => c.stdin === original.prompt).length, 1);
    equal(
      h.cli.filter((c) => c.stdin === pending.continuation!.prompt).length,
      1,
    );
    assert(
      h.writes.some(({ spec, value }) =>
        spec === "agent-task" && value.id === original.id &&
        value.status === "completed"
      ),
    );
  });
}

for (const continuing of [false, true]) {
  for (const phase of ["submitting", "waiting"] as const) {
    for (const savedEvidence of [false, true]) {
      Deno.test(`accepted ${continuing ? "continuation" : "original turn"} evidence in ${phase} recovers through ${savedEvidence ? "saved evidence" : "submission"} without another prompt`, async () => {
        const h = observedHarness();
        if (continuing) {
          h.turns.push({ response: { outcome: "completed" } });
          await rejects(
            () =>
              h.methods.run_agent.execute(
                { ...options, role: "triage" },
                h.context,
              ),
            "submission_required",
          );
        }
        h.turns.push({
          response: phase === "submitting"
            ? new Error("Lost provider response")
            : { outcome: "timed_out", acknowledged: true },
          submit: true,
        });
        const request = "Submit the findings already established";
        await rejects(
          () =>
            continuing
              ? h.methods.continue_agent.execute({
                taskId: AgentTaskSchema.parse(h.resources.get("agent-task")).id,
                request,
              }, h.context)
              : h.methods.run_agent.execute(
                { ...options, role: "triage" },
                h.context,
              ),
          phase === "submitting" ? "Lost provider response" : "timed_out",
        );
        const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
        const child = AgentStateSchema.parse(h.child.get("state"));
        equal(child.phase, phase);
        const submission = SubmissionSchema.parse(
          h.submissions.get(task.submission!.behaviorName)?.get("submission"),
        );
        assert(submission.status === "accepted");
        if (savedEvidence) {
          h.resources.set("agent-task", {
            ...task,
            status: "applying",
            next: submission.receipt.next,
            evidence: submission.receipt,
          });
        }
        const retry = () =>
          h.methods.continue_agent.execute(
            { taskId: task.id, request },
            h.context,
          );
        h.turns.push({ response: new Error("Observation response lost") });
        await rejects(retry, "Observation response lost");
        equal((await loadState(h.context)).phase, "triaging");
        equal(
          AgentTaskSchema.parse(h.resources.get("agent-task")).continuation,
          task.continuation,
        );
        h.turns.push({
          response: { outcome: "completed", message: "Truncated prose {" },
        });
        await retry();
        equal(
          AgentTaskSchema.parse(h.resources.get("agent-task")).status,
          "completed",
        );
        equal((await loadState(h.context)).phase, "classified");
        equal(
          h.cli.filter((c) => c.args[3] === "prompt").length,
          continuing ? 2 : 1,
        );
        equal(h.cli.filter((c) => c.args[3] === "spawn").length, 1);
        const observations = h.cli.filter((c) =>
          c.args[3] === "wait" && c.args.includes("--submission")
        );
        equal(observations.length, 2);
        for (const { args } of observations) {
          equal(args[args.indexOf("--submission") + 1], child.submissionId);
          equal(args[args.indexOf("--conversation") + 1], child.conversationId);
        }
      });
    }
  }
}

Deno.test("accepted evidence completes with no provider final text, including background completion", async () => {
  for (const message of [null, undefined, "", " \n\t"]) {
    for (const background of [false, true]) {
      const h = harness();
      h.controls.agentRunner = (args) =>
        Promise.resolve({
          code: 0,
          stderr: "",
          stdout: JSON.stringify(
            args[3] === "spawn"
              ? {
                outcome: "ok",
                spawned: { pane_id: "terminal_7", cwd: "/repo/control" },
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
                submission_id: args[3] === "prompt"
                  ? "submission-1"
                  : undefined,
                message: background && args[3] === "prompt"
                  ? "Working"
                  : message,
                background_tasks: background && args[3] === "prompt"
                  ? ["worker"]
                  : [],
              },
          ),
        });
      await h.methods.run_agent.execute(
        { ...options, role: "triage" },
        h.context,
      );
      const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
      equal(task.status, "completed");
      assert(task.evidence);
      equal(h.child.get("result")?.message, undefined);
    }
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
      h.child.set("state", {
        ...state,
        phase: "launching",
        launchId: undefined,
        launchReceipt: undefined,
        launchServer: undefined,
        launchRequestDigest: undefined,
      });
    } else {
      equal(state.phase, "launching");
    }
    await rejects(
      () => h.methods.resume_agent.execute({}, h.context),
      "",
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
    equal(cliCalls, response === responses.at(-1) ? 1 : 2);
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
      if (failure === "failTaskCompletionOnce") continue;
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
  h.controls.submitEvidence = false;
  h.controls.afterRun = () => h.child.clear();
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Interrupted",
  );
  const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const calls: string[][] = [];
  h.controls.agentRunner = recoveryRunner(false, calls);
  h.controls.submitEvidence = true;
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
    "superseded",
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
    case "plan_review_fixes":
      return {
        ...driverPlan,
        findingMappings: selectedRepairFindings(task.before).map((p) => ({
          finding: p.ref,
          steps: driverPlan.steps.map((s) => s.id),
          checks: driverPlan.checks.map((c) => c.id),
        })),
      };
    case "implement_review_fixes":
      return {
        summary: "Fixed selected review findings",
        commit: head,
        responses: selectedRepairFindings(task.before).map((p) => ({
          finding: p.ref,
          status: "addressed",
          rationale: "Fixture repair implemented",
          affectedPaths: ["lib.rs"],
          evidence: ["lib.rs:2"],
          checkIds: [],
        })),
      };
    case "review_ux":
      return {
        ...withUx(task.before).uxReview,
        priorFindings: priorImplementationFindings(task.before).filter((p) =>
          p.ref.role === "review_ux" && !p.fixed
        ).map((p) => ({
          finding: p.ref,
          outcome: "verified_fixed",
          rationale: "Fixture interaction verified",
          evidence: ["lib.rs:2"],
        })),
      };
    case "review_code":
      return {
        planVersion: task.before.plan!.version,
        commit: head,
        steps: [{ id: "fix", status: "implemented" }],
        priorFindings: priorImplementationFindings(task.before).filter((p) =>
          p.ref.role === "review_code" && !p.fixed
        ).map((p) => ({
          finding: p.ref,
          outcome: "verified_fixed",
          rationale: "Fixture repair verified",
          evidence: ["lib.rs:2"],
        })),
        findings: [],
        preliminaryChecks: [{
          command: "focused regression",
          result: "passed",
          summary: "Affected caller regression passes",
        }],
      };
  }
}

Deno.test("the Yinz review dimensions guide is available from the repository", async () => {
  const guide = await Deno.readTextFile(
    new URL(
      "../../../.agents/skills/issue-lifecycle/references/dimensions.md",
      import.meta.url,
    ),
  );
  assert(guide.startsWith("# Yinz review dimensions\n"));
});

for (
  const { role, prepared } of [
    { role: "review_plan", prepared: false },
    { role: "review_plan", prepared: true },
    { role: "review_code", prepared: true },
    { role: "review_ux", prepared: true },
  ] as const
) {
  Deno.test(`${role} reads Yinz dimensions from the ${prepared ? "control" : "worker"} checkout and retains the finding contract`, async () => {
    const origin = "/repo/issue";
    let state = prepared
      ? { ...implementing(), workerWorktree: origin }
      : transition(createState(issue, origin), "triage", triage);
    if (role === "review_plan") {
      state = transition(state, "plan", driverPlan);
    }
    if (role === "review_ux") {
      state = transition(state, "review_code", {
        planVersion: 1,
        commit: head,
        steps: [{ id: "fix", status: "implemented" }],
        findings: [],
      });
    }
    const h = harness(state);
    h.controls.evidenceForRole = driverEvidence;
    await h.methods.run_agent.execute({ ...options, role }, h.context);
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(task.config.cwd, origin);
    const references = `${
      prepared ? h.context.repoDir : origin
    }/.agents/skills/issue-lifecycle/references`;
    assert(
      task.prompt.includes(
        `Read ${references}/review.md and ${references}/dimensions.md before reviewing.`,
      ),
    );
    assert(
      task.prompt.includes(
        "cite a dimension name in description and its authoritative source in evidence alongside concrete affected code or plan locations",
      ),
    );
    assert(task.prompt.includes("do not add a category property"));
    assert(
      task.prompt.includes(
        "A dimension citation does not replace requirement, mechanism, source evidence, severity, finding identity, disposition or reopening rules",
      ),
    );
    assert(
      task.prompt.includes(
        "An inapplicable dimension creates no finding or additional testing obligation",
      ),
    );
    if (role === "review_plan") {
      assert(
        task.prompt.includes(
          `Read ${references}/planning.md and ${references}/review.md before working.`,
        ),
      );
      assert(
        task.prompt.includes("proposed behavior and verification coverage"),
      );
      assert(
        task.prompt.includes(
          "Every blocker needs an affected requirement, concrete failure mechanism and source evidence",
        ),
      );
      assert(
        task.prompt.includes(
          "speculative concerns alone are not high severity",
        ),
      );
      assert(task.prompt.includes("Reuse finding IDs for persistent concerns"));
      assert(
        task.prompt.includes(
          "Compare the previous plan, review findings, revision responses and current plan in planningContext",
        ),
      );
    } else {
      assert(
        task.prompt.includes("committed implementation and affected callers"),
      );
      assert(
        task.prompt.includes(
          role === "review_code"
            ? "Review the committed diff from the captured merge base to reviewed HEAD"
            : "Review the supplied committed change specifically",
        ),
      );
      assert(task.prompt.includes("Preliminary checks are advisory"));
      assert(task.prompt.includes(`The commit being reviewed is ${head}.`));
    }

    const contract = JSON.parse(
      task.prompt.split("Evidence schema: ")[1].split("\n\n")[0],
    );
    const finding = contract.properties.findings.items;
    equal(Object.keys(finding.properties).sort(), [
      "affectedPaths",
      "checkIds",
      "crossRoleLinks",
      "description",
      "evidence",
      "id",
      "mechanism",
      "origin",
      "reopening",
      "requirement",
      "resolution",
      "severity",
    ]);
    equal(finding.required, ["id", "severity", "description"]);
    equal(finding.additionalProperties, false);
  });
}

Deno.test("initial and revision planners retain planning and review guidance", async () => {
  const origin = "/repo/issue";
  for (const prepared of [false, true]) {
    const state = prepared
      ? { ...implementing(), workerWorktree: origin }
      : transition(createState(issue, origin), "triage", triage);
    const h = harness(state);
    h.controls.evidenceForRole = driverEvidence;
    await h.methods.run_agent.execute(
      { ...options, role: "plan" },
      h.context,
    );
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    const references = `${
      prepared ? h.context.repoDir : origin
    }/.agents/skills/issue-lifecycle/references`;
    assert(
      task.prompt.includes(
        `Read ${references}/planning.md and ${references}/review.md before working.`,
      ),
    );
    assert(
      task.prompt.includes("testingStrategy explaining the test selection"),
    );
    assert(
      task.prompt.includes("focused development checks selected per AGENTS.md"),
    );
    assert(
      task.prompt.includes(
        "Preserve unaffected steps, accepted decisions and tradeoffs",
      ),
    );
    assert(task.prompt.includes("Reuse finding IDs for persistent concerns"));
    if (prepared) {
      assert(
        task.prompt.includes(
          "This is a targeted revision. Include revision.summary, acceptedDecisions and a response (addressed, disputed with evidence, or unresolved) to each blocking finding",
        ),
      );
    } else {
      assert(
        task.prompt.includes(
          "Every blocker needs an affected requirement, concrete failure mechanism and source evidence",
        ),
      );
    }
  }
});

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

const savedChoice = { kind: "codex" as const, model: "custom", effort: "deep" };

for (const phase of ["triaging", "implementing"] as const) {
  Deno.test(`driver uses the saved worker choice for ${phase} roles and resumes`, async () => {
    const initial = phase === "triaging" ? createState(issue) : implementing();
    initial.workerAgent = savedChoice;
    const h = harness(initial);
    h.controls.evidenceForRole = driverEvidence;
    await h.drive.execute({ control: options.control }, h.context);
    equal(
      result(h).decision,
      phase === "triaging" ? "approve_plan" : "authorize_merge",
    );
    equal(
      dispatchedRoles(h),
      phase === "triaging"
        ? ["triage", "plan", "review_plan"]
        : ["implement", "review_code", "review_ux"],
    );
    for (const call of h.calls) {
      equal(call.arguments?.kind, savedChoice.kind);
      equal(call.arguments?.model, savedChoice.model);
      equal(call.arguments?.effort, savedChoice.effort);
      equal(call.arguments?.session, options.session);
    }
    equal((await loadState(h.context)).workerAgent, savedChoice);
    assert(
      result(h).presentation.includes(
        "Saved worker choice: codex; model: custom; effort: deep",
      ),
    );
    const count = h.calls.length;
    await h.drive.execute({}, h.context);
    equal(h.calls.length, count);
  });
}

Deno.test("saved worker choice requires session identity and accepts session/launch options without a repeated provider", async () => {
  const h = harness(createState(issue, undefined, savedChoice));
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("Worker session is required"));
  equal(h.calls.length, 0);
  await h.drive.execute({
    agent: {
      session: options.session,
      permissionMode: "custom-mode",
      timeoutSeconds: 42,
    },
    control: options.control,
  }, h.context);
  equal(result(h).decision, "approve_plan");
  for (const call of h.calls) {
    equal(call.arguments?.kind, "codex");
    equal(call.arguments?.model, "custom");
    equal(call.arguments?.permissionMode, "custom-mode");
    equal(call.arguments?.timeoutSeconds, 42);
  }
});

Deno.test("explicit drive provider settings override the saved choice and retain their replacement semantics", async () => {
  const h = harness(createState(issue, undefined, savedChoice));
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  for (const call of h.calls) {
    equal(call.arguments?.kind, "claude");
    equal(call.arguments?.model, undefined);
    equal(call.arguments?.effort, undefined);
  }
  equal((await loadState(h.context)).workerAgent, savedChoice);
});

Deno.test("pending workers retain their saved choice across interrupted controller recovery", async () => {
  const h = harness(createState(issue, undefined, savedChoice));
  h.controls.evidenceForRole = driverEvidence;
  const abort = new AbortController();
  h.context.signal = abort.signal;
  const run = h.context.runModel;
  h.context.runModel = () => {
    abort.abort(new Error("Interrupted saved-choice worker"));
    return Promise.reject(abort.signal.reason);
  };
  await rejects(
    () => h.drive.execute({ control: options.control }, h.context),
    "Interrupted saved-choice",
  );
  const pending = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(pending.config.model, savedChoice.model);
  h.context.signal = new AbortController().signal;
  h.context.runModel = run;
  await h.drive.execute(
    { agent: { ...options, model: "replacement" } },
    h.context,
  );
  equal(result(h).decision, "blocked");
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")), pending);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_plan");
  equal(h.calls[0].name, pending.behaviorName);
  equal(h.calls[0].method, "recover");
  equal(h.calls.length, 3);
  const resumed = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(resumed.config.kind, savedChoice.kind);
  equal(resumed.config.model, savedChoice.model);
});

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

Deno.test("a reserved revision retains the issue worker choice when future drive settings change", async () => {
  const h = harness(createState(issue, undefined, savedChoice));
  h.controls.evidenceForRole = blockedPlanEvidence;
  h.controls.failWrite = (spec, _name, value) =>
    spec === "agent-task" && value.status === "running" &&
    value.role === "plan" && (value.before as LifecycleState).planVersion === 1;
  await h.drive.execute({ control: options.control }, h.context);
  equal(result(h).decision, "blocked");
  const reserved =
    DriverSchema.parse(h.resources.get("driver")).automaticRevision!.assignment;
  equal(reserved.config.kind, savedChoice.kind);
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "authorize_plan_revision");
  const launch = h.calls.find((call) =>
    call.name === reserved.behaviorName && call.method === "run"
  )!;
  equal(launch.arguments?.kind, savedChoice.kind);
  equal(launch.arguments?.model, savedChoice.model);
  equal(launch.arguments?.effort, savedChoice.effort);
});

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
      0,
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
  equal(dispatchedRoles(h), ["implement", "review_code", "review_ux"]);
  equal(result(h).decision, "authorize_merge");
  equal(result(h).commit, head);
  assert(result(h).presentation.includes("Repaired the affected caller"));
  assert(result(h).presentation.includes("focused regression: passed"));
  equal((await loadState(h.context)).mergeAuthorization, undefined);
  const id = result(h).id;
  await h.drive.execute({}, h.context);
  equal(result(h).id, id);
  equal(h.calls.length, 3);
});

Deno.test("driver completes both reviews then asks which findings to address before running checks", async () => {
  const h = harness(implementing());
  h.controls.evidenceForRole = (task) =>
    task.role === "review_code"
      ? {
        ...driverEvidence(task) as object,
        findings: [{
          id: "bug",
          severity: "high",
          description: "Race remains",
          affectedPaths: ["lib.rs"],
          evidence: ["lib.rs:1"],
        }],
      }
      : driverEvidence(task);
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "address_review_findings");
  assert((await loadState(h.context)).uxReview);
  const handoffId = result(h).id;
  const calls = h.calls.length;
  await h.drive.execute({}, h.context);
  equal(result(h).id, handoffId);
  equal(h.calls.length, calls);
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
      "timeout-detail",
      "stale-turn",
      "stale-server",
      "stale-cwd",
      "startup",
      "dialog",
      "uncertain",
      "failure",
      "transport-after-timeout",
    ]
  ) {
    const h = harness();
    h.controls.evidenceForRole = driverEvidence;
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
            ...(scenario === "timeout-detail"
              ? { message: "Still executing original work" }
              : {}),
            ...(scenario === "stale-turn"
              ? {
                turn_id: args[3] === "prompt"
                  ? "original-turn"
                  : "replacement-turn",
              }
              : {}),
            ...(scenario === "stale-server"
              ? { server_instance: "replacement-server" }
              : {}),
            agents: [{
              ...agent,
              agent: {
                ...agent.agent,
                state: scenario === "dialog" ? "Waiting" : "Busy",
              },
              ...(scenario === "stale-cwd" ? { cwd: "/replacement" } : {}),
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
    if (["timeouts", "timeout-detail", "startup"].includes(scenario)) {
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
  equal(dispatchedRoles(h), [
    "implement",
    "review_code",
    "review_code",
    "review_ux",
  ]);
  assert(result(h).presentation.includes("Repaired the affected caller"));
  h.controls.repo.head = "c".repeat(40);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("review evidence is stale"));
  equal(h.calls.length, 4);
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
  equal(dispatchedRoles(h), [
    "implement",
    "review_code",
    "review_ux",
    "review_code",
    "review_ux",
  ]);
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
      "review_ux",
      "implement",
      "review_code",
      "review_ux",
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
      if (prompt) {
        h.controls.controlTab++;
        h.controls.evidenceForRole = undefined;
        h.controls.evidence = evidence;
      }
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
  equal(launched.length, count + 1);
  equal(launched.map((launch) => launch.role), [
    "triage",
    "plan",
    "review_plan",
    "plan",
    "review_plan",
    "implement",
    "review_code",
    "review_ux",
    "implement",
    "review_code",
    "review_ux",
    "review_code",
    "review_ux",
  ]);
  equal(
    launched.map((launch) => launch.tab),
    Array.from({ length: count + 1 }, (_, index) => String(index)),
  );
  equal(h.controls.placementCalls.length, count + 1);
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
  delete task.submission;
  delete task.evidence;
  h.submissions.clear();
  h.resources.set("agent-task", task);
  const child = AgentStateSchema.parse(h.child.get("state"));
  delete child.config.control;
  h.child.set("state", child);
  h.controls.runError = undefined;
  h.controls.evidenceForRole = driverEvidence;
  const control = { ...options.control, paneId: "terminal_2" };
  await h.drive.execute({ control }, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("typed submission supplement"));
  await h.methods.continue_agent.execute({
    taskId: task.id,
    request: "Submit original triage findings",
  }, h.context);
  await h.drive.execute({ control }, h.context);
  equal(result(h).decision, "approve_plan");
  equal(h.calls[1].method, "recover");
  equal(h.calls[1].arguments?.control, undefined);
  equal(h.calls.slice(3).map((call) => call.arguments?.control), [
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
    delete task.submission;
    delete task.evidence;
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
  // The child's message already names the blocked cleanup and Yinz's reason.
  await rejects(
    () =>
      h.methods.run_agent.execute(
        h.methods.run_agent.arguments.parse({ ...options, role: "triage" }),
        h.context,
      ).catch((error) => {
        equal(
          error.message,
          "Worker cleanup blocked: rejected: worker background work is still running",
        );
        throw error;
      }),
    "cleanup blocked",
  );
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
    assert(
      automatic.assignment.round?.assignmentId === automatic.assignment.id,
    );
    const rounds = h.resources.get("rounds") as {
      records: { assignmentId: string }[];
    };
    const persisted = new Set(
      h.writes.filter((w) => w.spec === "agent-task").map((w) => w.value.id),
    );
    equal(rounds.records.length, persisted.size);
    assert(
      rounds.records.every((r) => persisted.has(r.assignmentId)),
      "Interrupted reservation must not create a phantom round",
    );
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

Deno.test("revisions reject missing responses", async () => {
  {
    const h = harness(blockedInitialPlan(), driverPlan);
    await rejects(
      () =>
        h.methods.run_agent.execute({ ...options, role: "plan" }, h.context),
      "revision",
    );
    h.controls.runError = undefined;
    h.controls.evidence = driverPlan;
    await rejects(
      () => h.methods.resume_agent.execute({}, h.context),
      "revision",
    );
    equal((await loadState(h.context)).planVersion, 1);
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

Deno.test("a saved revision authorization resumes its original prompt", async () => {
  const h = harness();
  h.controls.evidenceForRole = blockedPlanEvidence;
  await h.drive.execute({ agent: options }, h.context);
  h.controls.failWrite = (spec) => spec === "agent-task";
  await h.drive.execute({ revisionApproval: revisionApproval(h) }, h.context);
  const driver = DriverSchema.parse(h.resources.get("driver"));
  const task = driver.revisionAuthorization!.assignment;
  task.prompt = "Original saved planner prompt";
  task.promptDigest = await promptDigest(task.prompt);
  h.resources.set("driver", driver);
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
    "review_ux",
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
  equal(h.calls.length, 8);
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
    equal(h.calls.map((call) => call.arguments?.cwd), [expected]);
    equal(
      AgentTaskSchema.parse(h.resources.get("agent-task")).config,
      original.config,
    );
    equal((await loadState(h.context)).workerWorktree, undefined);
  }
});

Deno.test("driver blocks a failed merge attempt with its evidence and keeps it listed after a later merge", async () => {
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
  const id = "d6da10b0-c980-4b82-b920-640238095f1a";
  state = transition(state, "begin_merge", {
    attempt: { id, originalHead: head, startedAt: timestamp },
  });
  const failure = {
    attemptId: id,
    at: timestamp,
    originalCommit: head,
    summary: "phases failed: test; tests: demo tests::flaky; exit 101",
    failure: "cargo x ci check failed",
    ciStatus: "failed",
    log: "/repo/.git/yinz-lifecycle/merges/d6da/ci-check.log",
    logResource: `merge-log-${id}`,
    exitCode: 101,
    phases: [
      { name: "lint", status: "passed" as const },
      { name: "test", status: "failed" as const },
    ],
    failedTests: ["demo tests::flaky"],
    failedTestsTruncated: 3,
  };
  state = recordMergeFailure(state, failure);
  const h = harness(state);
  await h.drive.execute({}, h.context);
  const blocked = result(h);
  equal(blocked.decision, "blocked");
  assert(
    blocked.reason.startsWith(
      `Merge attempt ${id} failed: ${failure.summary}; log: ${failure.log}. Read the log and run repair`,
    ),
    blocked.reason,
  );
  for (
    const text of [
      "Merge attempts:",
      "phases: lint passed, test failed",
      "failed tests: demo tests::flaky (+3 more in the log)",
      `log: ${failure.log}`,
      `name == "merge-log-${id}"`,
    ]
  ) {
    assert(blocked.presentation.includes(text), blocked.presentation);
  }
  equal(h.calls.length, 0);
  // After repair and a passing attempt, the earlier failure is still listed.
  const merged = transition(state, "merged", {
    receipt: {
      attemptId: id,
      originalCommit: head,
      rebasedCommit: "e".repeat(40),
      baseCommit: "b".repeat(40),
      mergedCommit: "e".repeat(40),
      worktree: "/repo/issue",
      branch: "issue-85",
      ciStatus: "passed",
      mergedAt: timestamp,
    },
  });
  const later = harness({
    ...merged,
    receipt: {
      ...merged.receipt!,
      attemptId: "e6da10b0-c980-4b82-b920-640238095f1a",
      previousAttempts: [
        {
          attemptId: "f6da10b0-c980-4b82-b920-640238095f1a",
          summary: "exit 1",
        },
        { attemptId: id, summary: failure.summary, log: failure.log },
      ],
    },
  });
  await later.drive.execute({}, later.context);
  const summary = result(later);
  equal(summary.decision, "summarize");
  for (
    const text of [
      "- f6da10b0-c980-4b82-b920-640238095f1a: failed; exit 1",
      `- ${id} at ${timestamp}: ${failure.summary}`,
      `- e6da10b0-c980-4b82-b920-640238095f1a: merged ${"e".repeat(40)}`,
    ]
  ) {
    assert(summary.presentation.includes(text), summary.presentation);
  }
  // A failure recorded in state is listed once, not again from the receipt.
  equal(summary.presentation.split(`- ${id}`).length, 2);
});

Deno.test("rollback can supersede a task interrupted before its evidence slot opened", async () => {
  const h = harness();
  h.controls.failEvidenceOpen = true;
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Evidence open interrupted",
  );
  equal(h.calls.length, 0);
  await h.methods.rollback.execute({
    step: "triage",
    request: "Supersede the interrupted launch",
  }, h.context);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "superseded",
  );
  equal(h.calls.length, 0);
});

Deno.test("pending discard fences accepted evidence before any recovery can import it", async () => {
  for (const fault of ["supersede", "final-save"]) {
    for (const recovery of ["resume", "continue", "discard", "drive"]) {
      const h = harness();
      h.controls.runError = "Lost provider response";
      await rejects(
        () =>
          h.methods.run_agent.execute(
            { ...options, role: "triage" },
            h.context,
          ),
        "Lost provider response",
      );
      const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
      const submission = () =>
        SubmissionSchema.parse(
          h.submissions.get(original.submission!.behaviorName)?.get(
            "submission",
          ),
        );
      equal(submission().status, "accepted");
      const discard = {
        request: "Discard the accepted but unapplied findings",
        confirmedStopped: true as const,
      };
      if (fault === "supersede") {
        h.controls.failEvidenceSupersede = true;
      } else {
        h.controls.failWrite = (spec, _name, value) =>
          spec === "agent-task" && value.status === "discarded";
      }
      await rejects(
        () => h.methods.discard_agent.execute(discard, h.context),
        fault === "supersede"
          ? "Evidence supersession interrupted"
          : "Interrupted recovery write",
      );
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).discard,
        discard,
      );
      equal(await loadState(h.context), original.before);
      const recover = () => {
        if (recovery === "continue") {
          return h.methods.continue_agent.execute({
            taskId: original.id,
            request: "Submit the existing findings",
          }, h.context);
        }
        if (recovery === "discard") {
          return h.methods.discard_agent.execute(discard, h.context);
        }
        if (recovery === "drive") return h.drive.execute({}, h.context);
        return h.methods.resume_agent.execute({}, h.context);
      };
      const calls = h.calls.length;
      if (fault === "supersede") {
        // Repeated failures must retain intent without touching the provider.
        for (let attempt = 0; attempt < 2; attempt++) {
          if (recovery === "drive") {
            await recover();
            equal(result(h).decision, "blocked");
            assert(
              result(h).reason.includes("Evidence supersession interrupted"),
            );
          } else await rejects(recover, "Evidence supersession interrupted");
          equal(h.calls.length, calls);
          equal(submission().status, "accepted");
          equal(await loadState(h.context), original.before);
          equal(
            AgentTaskSchema.parse(h.resources.get("agent-task")).discard,
            discard,
          );
        }
        h.controls.failEvidenceSupersede = false;
      }
      await recover();
      equal(submission().status, "superseded");
      equal(await loadState(h.context), original.before);
      assert(
        h.writes.some((write) =>
          write.spec === "agent-task" && write.value.id === original.id &&
          write.value.status === "discarded"
        ),
      );
      assert(!h.resources.has(`event-agent-${original.id}`));
      equal(h.controls.cleanupCalls, 0);
      // Drive may launch the next assignment after finishing the discard, but
      // must never resume or continue the discarded provider submission.
      assert(
        h.calls.slice(calls).every((call) =>
          call.name !== original.behaviorName &&
          call.definition !== original.behaviorName
        ),
      );
      if (recovery !== "drive") {
        equal(
          AgentTaskSchema.parse(h.resources.get("agent-task")).status,
          "discarded",
        );
        await h.methods.resume_agent.execute({}, h.context);
        equal(h.calls.length, calls);
      }
    }
  }
});

Deno.test("interrupted submission fencing preserves parent state and resumes before replacement dispatch", async () => {
  const h = harness();
  h.controls.runError = "Lost provider response";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Lost provider response",
  );
  const before = await loadState(h.context);
  h.controls.failEvidenceSupersede = true;
  await rejects(
    () =>
      h.methods.rollback.execute({
        step: "triage",
        request: "Supersede old work",
      }, h.context),
    "Evidence supersession interrupted",
  );
  equal(await loadState(h.context), before);
  await rejects(() => assertNoPendingAgent(h.context), "resume-rollback");
  h.controls.failEvidenceSupersede = false;
  await h.methods.resume_rollback.execute({}, h.context);
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "superseded",
  );
  equal(h.calls.length, 1);
});

Deno.test("every provider completes lifecycle work through the same typed Swamp receipt", async () => {
  for (const kind of ["claude", "codex", "muse", "pi", "agy"] as const) {
    const h = harness();
    h.controls.resultPatch = { message: "Arbitrary provider presentation" };
    await h.methods.run_agent.execute(
      { ...options, kind, role: "triage" },
      h.context,
    );
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(task.config.kind, kind);
    equal(task.evidence?.payload.role, "triage");
    equal(task.status, "completed");
    await h.methods.resume_agent.execute({}, h.context);
    equal(h.calls.length, 1);
  }
});

async function pendingLegacyProjection(
  role: "triage" | "implement" | "review_code",
) {
  const h = harness(
    role === "triage" ? createState(issue) : implementing(),
    role === "triage"
      ? triage
      : role === "implement"
      ? { summary: "Implemented", commit: head }
      : {
        planVersion: 1,
        commit: head,
        steps: [{ id: "fix", status: "implemented" }],
        findings: [],
      },
  );
  h.controls.failEventOnce = true;
  await rejects(
    () => h.methods.run_agent.execute({ ...options, role }, h.context),
    "event write failed",
  );
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  delete task.submission;
  delete task.evidence;
  task.message = "This legacy final chat is not JSON";
  h.resources.set("agent-task", task);
  // The saved projection is authoritative; neither freshness checks nor
  // reconciliation should need the original provider chat or state again.
  h.child.clear();
  return { ...h, task };
}

Deno.test("legacy applying projections import fresh evidence and reconcile applied evidence without chat", async () => {
  for (const role of ["triage", "review_code"] as const) {
    for (const applied of [false, true]) {
      const h = await pendingLegacyProjection(role);
      if (applied) {
        h.resources.set("state", h.task.next);
        h.controls.agenda = { ...issue, digest: "e".repeat(64) };
        h.controls.repo = {
          ...h.controls.repo,
          head: "c".repeat(40),
          branch: "another",
          clean: false,
        };
      }
      const agendaReads = h.controls.agendaReads;
      const calls = h.calls.length;
      await h.methods.resume_agent.execute({}, h.context);
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).status,
        "completed",
      );
      equal(await loadState(h.context), h.task.next);
      equal(h.controls.agendaReads, agendaReads + (applied ? 0 : 1));
      equal(
        h.writes.filter(({ spec }) => spec === "state").length,
        applied ? 0 : 1,
      );
      equal(h.calls.length, calls);
    }
  }
});

Deno.test("pending legacy projections reject changed requirements, worktrees and unverifiable commits", async () => {
  for (const role of ["triage", "implement", "review_code"] as const) {
    const faults = role === "review_code"
      ? [
        "agenda",
        "head",
        "dirty",
        "branch",
        "lifecycle",
        "missing commit",
        "different commit",
      ]
      : role === "implement"
      ? ["agenda", "unverifiable commit", "lifecycle"]
      : ["agenda", "lifecycle"];
    for (const fault of faults) {
      const h = await pendingLegacyProjection(role);
      if (fault === "agenda") {
        h.controls.agenda = { ...issue, digest: "e".repeat(64) };
      }
      if (fault === "head") h.controls.repo.head = "c".repeat(40);
      if (fault === "dirty") h.controls.repo.clean = false;
      if (fault === "branch") h.controls.repo.branch = "another";
      if (fault === "lifecycle") {
        h.resources.set("state", {
          ...h.task.before,
          lastFailure: "New lifecycle evidence",
        });
      }
      if (fault === "missing commit") delete h.task.next!.codeReview;
      if (fault === "different commit") {
        h.task.next!.codeReview!.commit = "c".repeat(40);
        h.controls.repo.head = "c".repeat(40);
      }
      if (fault === "unverifiable commit") {
        h.task.message = JSON.stringify({
          summary: "Legacy report",
          commit: head,
        });
      }
      h.resources.set("agent-task", h.task);
      const current = await loadState(h.context);
      const writes = h.writes.length;
      const calls = h.calls.length;
      await rejects(
        () => h.methods.resume_agent.execute({}, h.context),
        fault === "agenda"
          ? "requirements changed"
          : fault === "lifecycle"
          ? "Lifecycle changed"
          : fault.includes("commit")
          ? "no verifiable evidence commit"
          : "clean issue branch's current commit",
      );
      equal(await loadState(h.context), current);
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).status,
        "applying",
      );
      equal(h.writes.length, writes);
      equal(h.calls.length, calls);
      equal(
        h.writes.filter(({ spec }) => spec === "state" || spec === "event")
          .length,
        0,
      );
    }
  }
});

Deno.test("legacy implementation reports block pending review while accepted technical evidence proceeds to fresh UX review", async () => {
  for (const reviewed of [false, true]) {
    const h = harness(implementing(), { summary: "Implemented", commit: head });
    await h.methods.run_agent.execute(
      { ...options, role: "implement" },
      h.context,
    );
    const legacy = AgentTaskSchema.parse(h.resources.get("agent-task"));
    delete legacy.evidence;
    delete legacy.submission;
    legacy.message = "Truncated legacy implementation presentation {";
    h.resources.set("agent-task", legacy);
    h.resources.set("driver", { agent: options, implementation: legacy });
    if (reviewed) {
      h.resources.set(
        "state",
        transition(legacy.before, "review_code", {
          planVersion: 1,
          commit: head,
          steps: [{ id: "fix", status: "implemented" }],
          findings: [],
        }),
      );
    }
    h.controls.evidenceForRole = driverEvidence;
    const calls = h.calls.length;
    await h.drive.execute({}, h.context);
    equal(result(h).decision, reviewed ? "authorize_merge" : "blocked");
    if (!reviewed) {
      assert(result(h).reason.includes("typed submission supplement"));
    }
    assert(!result(h).presentation.includes("Truncated legacy"));
    equal(h.calls.length, calls + (reviewed ? 1 : 0));
  }
});

Deno.test("UX blockers and stale evidence stop all checks and are presented separately", async () => {
  for (const defect of ["high", "stale"] as const) {
    const state = withUx(
      transition(
        implementing(),
        "review_code",
        driverEvidence(
          { role: "review_code", before: implementing() } as ReturnType<
            typeof AgentTaskSchema.parse
          >,
        ),
      ),
    );
    if (defect === "high") {
      state.uxReview!.findings = [{
        id: "ux-loss",
        severity: "high",
        description: "Cancel hides draining work",
        evidence: ["cli.ts:42"],
      }];
    } else state.uxReview!.commit = "b".repeat(40);
    const h = harness(state);
    await h.drive.execute({ agent: options }, h.context);
    equal(
      result(h).decision,
      defect === "high" ? "address_review_findings" : "blocked",
    );
    equal(h.controls.checkCalls.length, 0);
    equal(h.calls.length, 0);
    assert(
      result(h).presentation.includes("UX assessment: no_material_impact"),
    );
    assert(result(h).presentation.includes("UX review findings"));
    assert(
      (defect === "high" ? result(h).presentation : result(h).reason).includes(
        defect === "high" ? "ux-loss" : "Current UX review",
      ),
    );
  }
});

Deno.test("UX accepted submission survives interrupted import and cleanup without duplicate dispatch", async () => {
  for (const fault of ["event", "cleanup", "task"] as const) {
    const state = transition(implementing(), "review_code", {
      planVersion: 1,
      commit: head,
      steps: [{ id: "fix", status: "implemented" }],
      findings: [],
    });
    const h = harness(state);
    h.controls.evidenceForRole = driverEvidence;
    h.controls.failEventOnce = fault === "event";
    h.controls.cleanupError = fault === "cleanup";
    h.controls.failTaskCompletionOnce = fault === "task";
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "blocked");
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(task.role, "review_ux");
    const original = {
      id: task.id,
      prompt: task.prompt,
      submission: task.submission,
    };
    h.controls.cleanupError = false;
    await h.drive.execute({}, h.context);
    equal(result(h).decision, "authorize_merge");
    equal(dispatchedRoles(h), ["review_ux"]);
    const completed = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal({
      id: completed.id,
      prompt: completed.prompt,
      submission: completed.submission,
    }, original);
    const id = result(h).id;
    await h.drive.execute({}, h.context);
    equal(result(h).id, id);
    assert(
      result(h).presentation.includes(
        "UX limitations: Source-based fixture assessment",
      ),
    );
  }
});

Deno.test("UX-only rollback and restart retain technical review and require new verification", async () => {
  const h = harness(implementing());
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  const technical = (await loadState(h.context)).codeReview;
  await h.methods.rollback.execute({
    step: "review_ux",
    request: "Repeat UX review",
  }, h.context);
  const rewound = await loadState(h.context);
  equal(rewound.codeReview, technical);
  assert(!rewound.uxReview && !rewound.verification);
  await h.drive.execute({}, h.context);
  equal(dispatchedRoles(h), [
    "implement",
    "review_code",
    "review_ux",
    "review_ux",
  ]);
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  await h.methods.restart_agent.execute({
    ...options,
    role: "review_ux",
    taskId: task.id,
    request: "Restart UX",
  }, h.context);
  await h.drive.execute({}, h.context);
  // Recovery saves its running replacement twice; child calls count launches.
  equal(h.calls.length, 5);
  equal((await loadState(h.context)).codeReview, technical);
  equal(result(h).decision, "authorize_merge");
});

Deno.test("UX worker failure missing submission and accepted-before-completion cannot advance", async () => {
  for (const fault of ["missing", "failed", "busy"] as const) {
    const state = transition(implementing(), "review_code", {
      planVersion: 1,
      commit: head,
      steps: [{ id: "fix", status: "implemented" }],
      findings: [],
    });
    const h = harness(state);
    h.controls.evidenceForRole = driverEvidence;
    if (fault === "missing") h.controls.submitEvidence = false;
    if (fault === "failed") h.controls.childOutcome = "failed";
    if (fault === "busy") h.controls.childPhase = "running";
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "blocked");
    equal(h.controls.checkCalls.length, 0);
    assert(!(await loadState(h.context)).uxReview);
    const first = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(first.role, "review_ux");
    await h.drive.execute({}, h.context);
    equal(AgentTaskSchema.parse(h.resources.get("agent-task")).id, first.id);
    equal(h.calls.filter((call) => call.method === "run").length, 1);
  }
});

Deno.test("legacy ready without UX blocks with recovery guidance and no worker or checks", async () => {
  let state = transition(implementing(), "review_code", {
    planVersion: 1,
    commit: head,
    steps: [{ id: "fix", status: "implemented" }],
    findings: [],
  });
  state = transition(verify(state), "ready", {
    commit: head,
    mergeRequest: "Old authorization",
  });
  delete state.uxReview;
  const h = harness(state);
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("rollback to review_ux and drive"));
  equal(h.calls.length, 0);
  equal(h.controls.checkCalls.length, 0);
});

async function reviewRepairHarness(
  workerAgent?: LifecycleState["workerAgent"],
) {
  const state = driverImplementing();
  if (workerAgent) state.workerAgent = workerAgent;
  const h = harness(state);
  h.controls.evidenceForRole = (task) => {
    const evidence = driverEvidence(task) as Record<string, unknown>;
    if (task.role === "review_code") {
      return {
        ...evidence,
        commit: h.controls.repo.head,
        priorFindings: priorImplementationFindings(task.before).filter((p) =>
          p.ref.role === "review_code" && !p.fixed
        ).map((p) => ({
          finding: p.ref,
          outcome: "persists",
          rationale: "Race still present in fixture",
          evidence: ["lib.rs:1"],
        })),
        findings: [{
          origin: priorImplementationFindings(task.before).find((p) =>
            p.ref.role === "review_code" && p.finding.id === "race"
          )?.ref,
          affectedPaths: ["lib.rs"],
          id: "race",
          severity: "critical",
          description: "Lost work",
          evidence: ["lib.rs:1"],
        }],
      };
    }
    if (task.role === "implement_review_fixes") {
      h.controls.repo.head = "d".repeat(40);
      return {
        ...evidence,
        summary: "Repaired race",
        commit: h.controls.repo.head,
      };
    }
    return evidence;
  };
  await h.drive.execute(
    workerAgent ? { control: options.control } : { agent: options },
    h.context,
  );
  equal(result(h).decision, "address_review_findings");
  const reviewed = await loadState(h.context);
  await saveState(
    h.context,
    "decide_review",
    transition(reviewed, "decide_review", {
      id: result(h).id,
      reviewFingerprint: codeReviewFingerprint(reviewed),
      request: "Plan a fix for the race",
      repair: [{ role: "review_code", findingId: "race" }],
      ignore: [],
    }),
  );
  return h;
}
Deno.test("review repair planners and implementers inherit the issue worker choice", async () => {
  const h = await reviewRepairHarness(savedChoice);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_review_fixes");
  const state = await loadState(h.context);
  await saveState(
    h.context,
    "approve_review_fixes",
    transition(state, "approve_review_fixes", {
      id: state.reviewDecisions!.at(-1)!.id,
      handoffId: result(h).id,
      request: "Implement the saved-choice repair plan",
    }),
  );
  await h.drive.execute({}, h.context);
  assert(dispatchedRoles(h).includes("plan_review_fixes"));
  assert(dispatchedRoles(h).includes("implement_review_fixes"));
  for (const call of h.calls) {
    equal(call.arguments?.kind, savedChoice.kind);
    equal(call.arguments?.model, savedChoice.model);
    equal(call.arguments?.effort, savedChoice.effort);
  }
  equal((await loadState(h.context)).workerAgent, savedChoice);
});
Deno.test("review repair loop plans only selected findings, waits for approval and repeats both reviews", async () => {
  const h = await reviewRepairHarness();
  const originalPlan = (await loadState(h.context)).plan;
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_review_fixes");
  let state = await loadState(h.context);
  assert(state.reviewDecisions!.at(-1)!.plan);
  equal(state.reviewDecisions!.at(-1)!.approval, undefined);
  const count = h.calls.length;
  const id = result(h).id;
  await h.drive.execute({}, h.context);
  equal(result(h).id, id);
  equal(h.calls.length, count);
  await saveState(
    h.context,
    "approve_review_fixes",
    transition(state, "approve_review_fixes", {
      id: state.reviewDecisions!.at(-1)!.id,
      handoffId: id,
      request: "Implement this repair plan",
    }),
  );
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "address_review_findings");
  equal(h.calls.length, count + 3); // implementation, technical review, UX review
  equal(h.controls.checkCalls.length, 0);
  state = await loadState(h.context);
  equal(state.plan, originalPlan);
  equal(state.codeReview!.commit, "d".repeat(40));
  equal(state.uxReview!.commit, "d".repeat(40));
  equal(state.reviewDecisions![0].result!.summary, "Repaired race");
  // Existing acceptance never silently covers new review evidence.
  await saveState(
    h.context,
    "decide_review",
    transition(state, "decide_review", {
      id: result(h).id,
      reviewFingerprint: codeReviewFingerprint(state),
      request: "Ignore the remaining race; merge with it outstanding",
      repair: [],
      ignore: [{ role: "review_code", findingId: "race" }],
    }),
  );
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_merge");
  equal(h.controls.checkCalls.length, 4); // approved issue and repair checks
  assert(
    result(h).presentation.includes(
      "Ignored (remain unresolved): review_code/race",
    ),
  );
  state = await loadState(h.context);
  await saveState(
    h.context,
    "ready",
    transition(state, "ready", {
      commit: state.codeReview!.commit,
      mergeRequest: "Merge with the outstanding race",
    }),
  );
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "integrate");
});
Deno.test("review repair evidence recovers interrupted imports without dispatching a duplicate worker", async () => {
  for (const role of ["plan_review_fixes", "implement_review_fixes"] as const) {
    const h = await reviewRepairHarness();
    if (role === "implement_review_fixes") {
      await h.drive.execute({}, h.context);
      const state = await loadState(h.context);
      await saveState(
        h.context,
        "approve_review_fixes",
        transition(state, "approve_review_fixes", {
          id: state.reviewDecisions!.at(-1)!.id,
          handoffId: result(h).id,
          request: "Implement",
        }),
      );
    }
    h.controls.failEventOnce = true;
    await h.drive.execute({}, h.context);
    equal(result(h).decision, "blocked");
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(task.role, role);
    const count = h.calls.length;
    await h.drive.execute({}, h.context);
    equal(h.calls.length, count + (role === "implement_review_fixes" ? 2 : 0));
    equal(
      result(h).decision,
      role === "plan_review_fixes"
        ? "approve_review_fixes"
        : "address_review_findings",
    );
  }
});
Deno.test("explicit restart preserves selected repair scope and approved implementation authority", async () => {
  for (const role of ["plan_review_fixes", "implement_review_fixes"] as const) {
    const h = await reviewRepairHarness();
    if (role === "implement_review_fixes") {
      await h.drive.execute({}, h.context);
      const state = await loadState(h.context);
      await saveState(
        h.context,
        "approve_review_fixes",
        transition(state, "approve_review_fixes", {
          id: state.reviewDecisions!.at(-1)!.id,
          handoffId: result(h).id,
          request: "Implement",
        }),
      );
    }
    h.controls.runError = "Provider unavailable";
    await h.drive.execute({}, h.context);
    const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(old.role, role);
    h.controls.runError = undefined;
    await h.methods.restart_agent.execute({
      ...options,
      role,
      taskId: old.id,
      request: "Restart this repair worker",
    }, h.context);
    const state = await loadState(h.context);
    const fix = state.reviewDecisions!.at(-1)!;
    equal(fix.repair, [{ role: "review_code", findingId: "race" }]);
    if (role === "implement_review_fixes") {
      equal(fix.approval, "Implement");
      assert(fix.result);
    } else assert(fix.plan);
    await h.drive.execute({}, h.context);
    equal(
      result(h).decision,
      role === "plan_review_fixes"
        ? "approve_review_fixes"
        : "address_review_findings",
    );
  }
});

Deno.test("explicit repair restart recovers imported scope after cleanup failure", async () => {
  for (const role of ["plan_review_fixes", "implement_review_fixes"] as const) {
    const h = await reviewRepairHarness();
    if (role === "implement_review_fixes") {
      await h.drive.execute({}, h.context);
      const state = await loadState(h.context);
      await saveState(
        h.context,
        "approve_review_fixes",
        transition(state, "approve_review_fixes", {
          id: state.reviewDecisions!.at(-1)!.id,
          handoffId: result(h).id,
          request: "Implement",
        }),
      );
    }
    h.controls.cleanupError = true;
    await h.drive.execute({}, h.context);
    equal(result(h).decision, "blocked");
    const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(old.role, role);
    equal(old.status, "completed");
    equal(old.cleanup, "pending");
    const imported = await loadState(h.context);
    if (role === "implement_review_fixes") {
      assert(imported.reviewDecisions!.at(-1)!.result);
      equal(imported.codeReview, undefined);
      equal(imported.uxReview, undefined);
    }
    const args = {
      ...options,
      role,
      taskId: old.id,
      request: "Restart this repair worker after cleanup failed",
    };
    await rejects(
      () =>
        h.methods.restart_agent.execute({
          ...args,
          taskId: "00000000-0000-4000-8000-000000000000",
        }, h.context),
      "current assignment",
    );
    if (role === "implement_review_fixes") {
      await rejects(
        () =>
          h.methods.restart_agent.execute({
            ...args,
            role: "plan_review_fixes",
          }, h.context),
        "No current review repair assignment",
      );
    }
    const count = h.calls.length;
    const cleanupCount = h.controls.cleanupCalls;
    h.controls.cleanupError = false;
    await h.methods.restart_agent.execute(args, h.context);
    const replacement = AgentTaskSchema.parse(h.resources.get("agent-task"));
    assert(replacement.id !== old.id);
    equal(replacement.role, role);
    equal(replacement.status, "completed");
    equal(h.calls.length, count + 1);
    equal(h.controls.cleanupCalls, cleanupCount + 1);
    const archived = AgentTaskSchema.parse(
      h.resources.get(`agent-task-${old.id}`),
    );
    equal(archived.status, "superseded");
    equal(archived.cleanup, undefined);
    const next = replacement.before;
    const fix = next.reviewDecisions!.at(-1)!;
    equal(next.reviewDecisions!.slice(0, -1), imported.reviewDecisions);
    equal(next.codeReview, old.before.codeReview);
    equal(next.uxReview, old.before.uxReview);
    equal(next.implementation!.head, old.before.implementation!.head);
    equal(fix.repair, old.before.reviewDecisions!.at(-1)!.repair);
    equal(fix.reviewFingerprint, codeReviewFingerprint(next));
    equal(fix.result, undefined);
    equal(
      fix.plan,
      role === "implement_review_fixes"
        ? old.before.reviewDecisions!.at(-1)!.plan
        : undefined,
    );
    equal(
      fix.approval,
      role === "implement_review_fixes" ? "Implement" : undefined,
    );
    const completed = await loadState(h.context);
    await h.methods.restart_agent.execute(args, h.context);
    equal(h.calls.length, count + 1);
    equal(await loadState(h.context), completed);
    await rejects(
      () =>
        h.methods.restart_agent.execute({
          ...args,
          request: "Different restart of the old task",
        }, h.context),
      "current assignment",
    );
    await h.drive.execute({}, h.context);
    equal(
      result(h).decision,
      role === "plan_review_fixes"
        ? "approve_review_fixes"
        : "address_review_findings",
    );
  }
});

Deno.test("legacy ready state with outstanding findings returns to user direction before integration", async () => {
  const reviewed = transition(implementing(), "review_code", {
    planVersion: 1,
    commit: head,
    steps: [{ id: "fix", status: "implemented" }],
    findings: [],
  });
  const state = transition(verify(reviewed), "ready", {
    commit: head,
    mergeRequest: "Old merge authorization",
  });
  state.codeReview!.findings = [{
    id: "legacy",
    severity: "medium",
    description: "Previously advisory finding",
  }];
  const h = harness(state);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "address_review_findings");
  equal(h.calls.length, 0);
  equal(h.controls.checkCalls.length, 0);
  const current = await loadState(h.context);
  await saveState(
    h.context,
    "decide_review",
    transition(current, "decide_review", {
      id: result(h).id,
      reviewFingerprint: codeReviewFingerprint(current),
      request: "Accept legacy finding and merge",
      repair: [],
      ignore: [{ role: "review_code", findingId: "legacy" }],
    }),
  );
  equal((await loadState(h.context)).phase, "implementing");
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_merge");
});

Deno.test("driver closeout renewed consent and declined partial cleanup reach the correct terminal handoffs", async () => {
  for (const decision of ["offered", "declined"] as const) {
    for (const git of ["pending", "complete"] as const) {
      const initial = createState(issue);
      initial.phase = "done";
      initial.outcome = { outcomeMet: true, summary: "Delivered" };
      const identity = { device: "1", inode: "2" };
      initial.closeout = {
        id: crypto.randomUUID(),
        offerId: crypto.randomUUID(),
        issueDigest: issue.digest,
        attemptId: crypto.randomUUID(),
        origin: {
          session: "development",
          paneId: "terminal_1",
          server_instance: "server",
          provider: "codex",
          conversation: "original",
        },
        fence: {
          server_instance: "server",
          pane_id: "terminal_1",
          provider: "codex",
          conversation: "original",
          input_revision: 1,
          turn: "turn",
        },
        target: {
          worktree: "/repo/issue",
          branch: "issue-85",
          head,
          gitDir: "/repo/.git/worktrees/issue",
          branchIdentity: "ref",
          directoryIdentity: identity,
          registrationIdentity: identity,
          linkIdentity: identity,
          gitLink: "gitdir: /repo/.git/worktrees/issue",
          owner: {
            issueKey: "default/85",
            worktree: "/repo/issue",
            branch: "issue-85",
          },
        },
        decision,
        git,
        pane: "expired",
        authorizedRequest: "Original bundled authorization",
      };
      if (decision === "offered") {
        for (const authorized of [false, true]) {
          for (const stage of ["cleanup preview", "Git cleanup"]) {
            const contended = structuredClone(initial);
            if (!authorized) delete contended.closeout;
            else contended.closeout!.decision = "accepted";
            const reason =
              `Optional closeout deferred during ${stage}: worktree ownership lock is busy; another lifecycle operation holds the brief worktree ownership guard. Retry closeout or resume-closeout after it finishes.`;
            const busy = harness(contended, triage, () => {
              throw new GitOperationError(reason, {
                code: "lock_busy",
                stage,
                lock: "worktree ownership",
              });
            });
            const beforeBusy = await loadState(busy.context);
            await busy.drive.execute({}, busy.context);
            equal(result(busy).decision, authorized ? "blocked" : "complete");
            assert(result(busy).reason.includes(reason));
            equal(await loadState(busy.context), beforeBusy);
            equal(dispatchedRoles(busy), []);
          }
        }
      }
      const h = harness(initial);
      await h.drive.execute({}, h.context);
      equal(
        result(h).decision,
        decision === "declined" ? "complete" : "authorize_closeout",
      );
      if (decision === "offered") equal(result(h).id, initial.closeout.offerId);
      const before = result(h);
      await h.drive.execute({}, h.context);
      equal(result(h), before);
      equal(dispatchedRoles(h), []);
      if (git === "complete" && decision === "offered") {
        assert(before.reason.includes("already removed"));
        assert(before.reason.includes("Decline keeps that pane"));
        // Recovery can reconcile exact absence while preparing its new offer.
        initial.closeout.decision = "invalidated";
        const recovered = harness(initial, triage, async (context, state) => {
          state.closeout!.pane = "absent";
          state.closeout!.decision = "offered";
          await saveState(context, "offer_closeout_recovery", state);
          return { state, dataHandles: [] };
        });
        await recovered.drive.execute({}, recovered.context);
        equal(result(recovered).decision, "complete");
      }
    }
  }
});

Deno.test("issue85 accepted original assignment imports once through closed submission proof", async () => {
  const h = observedHarness();
  h.turns.push({
    response: { outcome: "status_unavailable", acknowledged: true },
    submit: true,
  });
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "status_unavailable",
  );
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const child = AgentStateSchema.parse(h.child.get("state"));
  assert(child.launchServer);
  h.turns.push({
    response: {
      outcome: "completed",
      agents: [],
      acknowledged: true,
      prompt_delivery: "started",
      server_instance: child.launchServer,
      turn_id: null,
      retained_submission: {
        server_instance: child.launchServer,
        session: task.config.session,
        pane_id: child.paneId,
        kind: task.config.kind,
        cwd: task.config.cwd,
        conversation_id: child.conversationId,
        submission_id: child.submissionId,
        turn_id: null,
        prompt_id: "original-provider-prompt",
        acknowledged: true,
        prompt_delivery: "started",
        outcome: "completed",
        message: "Finished",
        answered: true,
        quiescent: true,
      },
    },
  });
  h.controls.failEventOnce = true;
  await rejects(() => h.methods.resume_agent.execute({}, h.context), "");
  await h.methods.resume_agent.execute({}, h.context);
  await h.methods.resume_agent.execute({}, h.context);
  equal((await loadState(h.context)).phase, "classified");
  equal(
    AgentTaskSchema.parse(h.resources.get("agent-task")).status,
    "completed",
  );
  equal(h.cli.filter((c) => c.args[3] === "spawn").length, 1);
  equal(h.cli.filter((c) => c.args[3] === "prompt").length, 1);
  equal(
    h.cli.filter((c) => c.args[3] === "wait" && c.args.includes("--submission"))
      .length,
    1,
  );
});

// These consumer fixtures use the native retained response shape. Screen tests produce
// that shape through passive-report, SessionEnd and composed-close paths;
// no receipt creates a proof.
for (const continuing of [false, true]) {
  for (const closure of ["passive-close", "session-end", "composed-close"]) {
    for (const fault of ["child-result", "parent-event"]) {
      Deno.test(`issue85 accepted implementation ${continuing ? "continuation" : "original"} imports once after ${closure} and ${fault}`, async () => {
        const initial = implementing();
        const h = observedHarness(initial, {
          summary: "Implemented",
          commit: head,
        });
        if (continuing) {
          h.turns.push({ response: { outcome: "completed" } });
          await rejects(
            () =>
              h.methods.run_agent.execute(
                { ...options, role: "implement" },
                h.context,
              ),
            "submission_required",
          );
        }
        h.turns.push({
          response: {
            outcome: "status_unavailable",
            acknowledged: true,
            turn_id: "native-turn",
          },
          submit: true,
        });
        const taskBefore = continuing
          ? AgentTaskSchema.parse(h.resources.get("agent-task"))
          : undefined;
        await rejects(
          () =>
            continuing
              ? h.methods.continue_agent.execute({
                taskId: taskBefore!.id,
                request: "Submit existing work",
              }, h.context)
              : h.methods.run_agent.execute(
                { ...options, role: "implement" },
                h.context,
              ),
          "status_unavailable",
        );
        const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
        const child = AgentStateSchema.parse(h.child.get("state"));
        const receipt = SubmissionSchema.parse(
          h.submissions.get(task.submission!.behaviorName)?.get("submission"),
        );
        assert(receipt.status === "accepted");
        h.turns.push({
          response: {
            outcome: "completed",
            agents: [],
            acknowledged: true,
            prompt_delivery: "started",
            server_instance: child.launchServer,
            turn_id: child.turnId,
            retained_submission: {
              server_instance: child.launchServer,
              session: task.config.session,
              pane_id: child.paneId,
              kind: task.config.kind,
              cwd: task.config.cwd,
              conversation_id: child.conversationId,
              submission_id: child.submissionId,
              turn_id: child.turnId,
              prompt_id: "original-provider-prompt",
              acknowledged: true,
              prompt_delivery: "started",
              outcome: "completed",
              message: "Finished",
              answered: true,
              quiescent: true,
            },
          },
        });
        h.controls.failChildResultOnce = fault === "child-result";
        h.controls.failEventOnce = fault === "parent-event";
        await rejects(
          () => h.methods.resume_agent.execute({}, h.context),
          fault === "child-result"
            ? "child publication interrupted"
            : "event write failed",
        );
        await h.methods.resume_agent.execute({}, h.context);
        const imported = AgentTaskSchema.parse(h.resources.get("agent-task"));
        await h.methods.resume_agent.execute({}, h.context);
        equal(AgentTaskSchema.parse(h.resources.get("agent-task")), imported);
        equal(imported.id, task.id);
        equal(imported.promptDigest, task.promptDigest);
        equal(imported.status, "completed");
        equal(imported.evidence, receipt.receipt);
        equal(h.controls.repo.head, head);
        equal(await loadState(h.context), initial);
        equal(h.cli.filter((c) => c.args[3] === "spawn").length, 1);
        equal(
          h.cli.filter((c) => c.args[3] === "prompt").length,
          continuing ? 2 : 1,
        );
        equal(
          h.cli.filter((c) =>
            c.args[3] === "wait" && c.args.includes("--submission")
          ).length,
          1,
        );
        equal(h.writes.filter(({ spec }) => spec === "event").length, 1);
        equal(h.controls.cleanupCalls, 1);
        equal(
          AgentStateSchema.parse(h.child.get("state")).submissionId,
          child.submissionId,
        );
      });
    }
  }
}

Deno.test("issue85 accepted implementation receipt never promotes positively interrupted execution", async () => {
  const initial = implementing();
  const h = observedHarness(initial, { summary: "Implemented", commit: head });
  h.turns.push({
    response: {
      outcome: "status_unavailable",
      acknowledged: true,
      turn_id: "native-turn",
    },
    submit: true,
  });
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "implement" }, h.context),
    "status_unavailable",
  );
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  for (let attempt = 0; attempt < 2; attempt++) {
    h.turns.push({
      response: {
        outcome: "interrupted",
        acknowledged: true,
        prompt_delivery: "started",
        message: "The turn was interrupted.",
      },
    });
    await rejects(
      () => h.methods.resume_agent.execute({}, h.context),
      "interrupted",
    );
    equal(await loadState(h.context), initial);
    equal(AgentTaskSchema.parse(h.resources.get("agent-task")).id, task.id);
    assert(
      AgentTaskSchema.parse(h.resources.get("agent-task")).status !==
        "completed",
    );
    equal(h.controls.repo.head, head);
    equal(h.writes.filter(({ spec }) => spec === "event").length, 0);
  }
  equal(h.cli.filter((c) => c.args[3] === "prompt").length, 1);
  equal(h.cli.filter((c) => c.args[3] === "spawn").length, 1);
});

Deno.test("late durable completion reconciles after a lost controller response without invoking the worker", async () => {
  const h = harness();
  h.controls.runError = "Controller lost child response";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Controller lost",
  );
  const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const runModel = h.context.runModel;
  h.context.runModel = (request) => {
    if (request.modelType === "@yinz/agent" && request.method !== "cleanup") {
      throw new Error("Completed worker must not be observed again");
    }
    return runModel(request);
  };
  h.controls.runError = undefined;
  await h.methods.resume_agent.execute({}, h.context);
  const writes = h.writes.length;
  await h.methods.resume_agent.execute({}, h.context);
  equal(h.writes.length, writes);
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).id, original.id);
  equal((await loadState(h.context)).phase, "classified");
  equal(h.calls.map((call) => call.method), ["run"]);
});

Deno.test("accepted evidence with missing original child state cannot launch a recovery worker", async () => {
  const h = harness();
  h.controls.runError = "Controller interrupted";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Controller interrupted",
  );
  h.child.delete("state");
  const calls = h.calls.length;
  await rejects(
    () => h.methods.resume_agent.execute({}, h.context),
    "no durable original worker state",
  );
  equal(h.calls.length, calls);
  equal((await loadState(h.context)).phase, "triaging");
});

Deno.test("continuity journals one reservation per assignment and keeps review attribution distinct from launch triggers", async () => {
  const h = harness();
  await h.methods.run_agent.execute({ ...options, role: "triage" }, h.context);
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(task.contractVersion, 2);
  assert(task.continuity?.assignmentId === task.id);
  const before = JSON.stringify(h.resources.get("rounds"));
  await h.methods.resume_agent.execute({}, h.context);
  equal(JSON.stringify(h.resources.get("rounds")), before);
  const journal = h.resources.get("rounds") as {
    records: {
      assignmentId: string;
      predecessor?: string;
      launchCauses: { kind: string }[];
      corrections: unknown[];
    }[];
  };
  equal(journal.records.length, 1);
  equal(journal.records[0].launchCauses[0].kind, "initial_work");
  const repairs = await reviewRepairHarness();
  await repairs.drive.execute({}, repairs.context);
  const planned = await loadState(repairs.context);
  await saveState(
    repairs.context,
    "approve_review_fixes",
    transition(planned, "approve_review_fixes", {
      id: planned.reviewDecisions!.at(-1)!.id,
      handoffId: result(repairs).id,
      request: "Implement the exact repairs",
    }),
  );
  await repairs.drive.execute({}, repairs.context);
  const rounds = (repairs.resources.get("rounds") as typeof journal).records;
  equal(new Set(rounds.map((r) => r.assignmentId)).size, rounds.length);
  assert(
    rounds.slice(1).every((r, i) => r.predecessor === rounds[i].assignmentId),
  );
  const review = rounds.findLast((r) =>
    JSON.stringify(r.corrections).includes("incomplete_repair")
  );
  assert(
    review,
    "Persisting selected finding must record incomplete repair evidence",
  );
  assert(
    !JSON.stringify(review.corrections).includes("introducedByRepair"),
    "Chronology cannot establish introduction",
  );
  const saved = JSON.stringify(repairs.resources.get("rounds"));
  await repairs.methods.resume_agent.execute({}, repairs.context);
  equal(JSON.stringify(repairs.resources.get("rounds")), saved);
});
