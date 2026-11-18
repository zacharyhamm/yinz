import { launchReply, launchSupport } from "./launch_fixtures.ts";
import { assignmentFileContents } from "../_lib/assignment_files.ts";
import {
  createEvidenceMethods,
  type EvidenceContext,
} from "../issue_evidence.ts";
import {
  assignmentCore,
  evidenceDigest,
  submissionInstructions,
  SubmissionSchema,
  SubmitArgsSchema,
} from "../_lib/issue_evidence.ts";
import {
  AgentTaskSchema,
  assertNoPendingAgent,
  createAgentMethods,
  type IssueAgentContext,
  type IssueAgentDependencies,
  RecoverySchema,
  taskPrompt,
} from "../_lib/issue_agents.ts";
import {
  adoptWorkerWorktree,
  createDriveMethod,
  DriverSchema,
  HandoffSchema,
} from "../_lib/issue_driver.ts";
import { nextStep } from "../_lib/handoff_next.ts";
import { RECOVERY_DEADLINE_MS } from "../_lib/worker_recovery.ts";
import {
  codeReviewFingerprint,
  createState,
  effectivePlanFindings,
  initialPlanning,
  type LifecycleState,
  priorFindingObligations,
  priorImplementationFindings,
  recordMergeFailure,
  refreshIssue,
  selectedRepairFindings,
  transition,
} from "../_lib/lifecycle.ts";
import { GitOperationError, loadState, saveState } from "../_lib/runtime.ts";
import {
  NO_VERIFICATION_EXTRAS,
  type VerificationSettings,
} from "../_lib/verification.ts";
import {
  AgentConfigSchema,
  type AgentContext,
  AgentStateSchema,
  confirmFailureAgent,
  confirmStoppedAgent,
  continueAgent,
  promptDigest,
  recoverAgent,
  runAgent,
  type YinzRunner,
} from "../_lib/yinz_agent.ts";

type SavedTask = ReturnType<typeof AgentTaskSchema.parse>;
/** One contract-3 assignment file, exactly as publication would write it. */
function assignmentText(
  task: SavedTask,
  key: Parameters<ReturnType<typeof assignmentFileContents>["get"]>[0],
) {
  const bytes = assignmentFileContents({
    ...task,
    submission: task.submission!,
  })
    .get(key);
  assert(bytes, `assignment has no ${key} file`);
  return new TextDecoder().decode(bytes);
}
/** What a contract-3 worker reads: its prompt plus the bound packet file. */
function workerMaterial(task: SavedTask) {
  return task.assignmentFiles
    ? `${task.prompt}\n\n${assignmentText(task, "packet")}`
    : task.prompt;
}
/** The saved task a runModel call delivered, from the agent-task write log. */
function taskOfCall(
  h: { writes: { spec: string; value: Record<string, unknown> }[] },
  call: { name?: string },
) {
  const write = h.writes.findLast((w) =>
    w.spec === "agent-task" && w.value.behaviorName === call.name
  );
  assert(write, `no saved task for ${call.name}`);
  return AgentTaskSchema.parse(write.value);
}

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
    // Each entry replaces runError for one worker call, after the child wrote.
    runErrorQueue: [] as string[],
    // Each entry is one structured child cleanup refusal, saved before it throws.
    cleanupRefusals: [] as {
      outcome: string;
      message: string;
      retryable: boolean;
    }[],
    // The simulated child's confirm_stopped observation outcome, or refusal.
    stopOutcome: undefined as string | undefined,
    stopError: undefined as string | undefined,
    sleeps: [] as number[],
    clock: 0,
    reopenError: undefined as string | undefined,
    reopened: 0,
    prepareCalls: [] as Record<string, unknown>[],
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
    // Fixtures opt out of baseline checks and the probe unless a test sets them.
    verificationSettings: (): Promise<VerificationSettings> =>
      Promise.resolve(NO_VERIFICATION_EXTRAS),
    rebaseProbe: (_request: Record<string, unknown>): unknown => ({
      main: "b".repeat(40),
      status: "clean",
    }),
    // Every Git helper action, in call order.
    gitActions: [] as string[],
    // When set, review-context reports a descendant endpoint delta that
    // changes these paths, as the real helper does after a repair commit.
    deltaPaths: undefined as string[] | undefined,
    agentRunner: undefined as YinzRunner | undefined,
    agentId: () => "submission-1",
    agendaReads: 0,
    controlTab: 0,
    placementCalls: [] as string[][],
    notifications: [] as string[][],
    // Each entry replaces one successful control-pane listing.
    placementFailures: [] as { code: number; stdout: string; stderr: string }[],
    afterRun: () => {},
    // Every published contract-3 task, in call order.
    publishCalls: [] as ReturnType<typeof AgentTaskSchema.parse>[],
    failPublish: undefined as string | undefined,
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
        const refusal = controls.cleanupRefusals.shift();
        if (refusal) {
          const thrown =
            `Worker cleanup blocked: ${refusal.outcome}: ${refusal.message}`;
          const state = AgentStateSchema.parse(child.get("state"));
          child.set("state", {
            ...state,
            cleanupRefusal: {
              ...refusal,
              thrown,
              ...(state.submissionId
                ? { submissionId: state.submissionId }
                : {}),
              ...(state.conversationId
                ? { conversationId: state.conversationId }
                : {}),
              observedAt: timestamp,
            },
          });
          return { ok: false, error: { message: thrown } };
        }
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
              const failure = controls.placementFailures.shift();
              if (failure) return Promise.resolve(failure);
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
          } else if (request.method === "confirm_stopped") {
            await confirmStoppedAgent(agentContext, dependencies);
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
      if (request.method === "confirm_stopped") {
        if (controls.stopError) {
          return { ok: false, error: { message: controls.stopError } };
        }
        child.set("observation", {
          paneId: "terminal_7",
          outcome: controls.stopOutcome ?? "ok",
          rawResponse: "{}",
          observedAt: timestamp,
        });
        return { ok: true };
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
      const runError = controls.runErrorQueue.shift() ?? controls.runError;
      return runError ? { ok: false, error: { message: runError } } : {
        ok: true,
      };
    },
  };
  const dependencies: IssueAgentDependencies = {
    notificationRunner: (args) => {
      assert(resources.has("handoff"), "Persist handoff before notifying");
      controls.notifications.push(args);
      return Promise.resolve({ code: 0, stdout: "", stderr: "" });
    },
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
      controls.gitActions.push(String(request.action));
      if (request.action === "rebase-probe") {
        try {
          return Promise.resolve(controls.rebaseProbe(request));
        } catch (error) {
          return Promise.reject(error);
        }
      }
      if (request.action === "review-context") {
        const previous = (request.previous as { head?: string } | undefined)
          ?.head;
        const delta = controls.deltaPaths && previous &&
          previous !== controls.repo.head;
        return Promise.resolve({
          main: controls.repo.main,
          mergeBase: controls.repo.main,
          head: controls.repo.head,
          featureDiff: "fixture full feature diff",
          ...(delta
            ? {
              previousHead: previous,
              delta: "fixture repair diff",
              deltaPaths: controls.deltaPaths,
            }
            : {}),
          ancestry: delta ? "descendant" : "initial",
          baselineChanged: false,
          limitations: [],
        });
      }
      if (request.action === "prepare") {
        controls.prepareCalls.push(request);
        return Promise.resolve({
          ...controls.repo,
          worktree: request.worktree,
          branch: request.branch,
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
    verificationSettings: () => controls.verificationSettings(),
    publishAssignment: (task) => {
      controls.publishCalls.push(structuredClone(task));
      return controls.failPublish
        ? Promise.reject(new Error(controls.failPublish))
        : Promise.resolve();
    },
    sleep: (ms) => {
      controls.sleeps.push(ms);
      controls.clock += ms;
      return Promise.resolve();
    },
    monotonicNow: () => controls.clock,
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
  // The server's receipt proves the stop, so no attestation is needed.
  const discard = {
    request: "Discard the rejected launch so I can use supported effort",
  };
  for (
    const invalid of [
      { ...discard, request: " " },
      { ...discard, confirmedStopped: false },
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
  assert(AgentStateSchema.parse(h.child.get("state")).launchReceipt);
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).discard, {
    ...discard,
    basis: "rejected_launch",
    stopEvidence: "launch_receipt",
  });
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
  // The correlated failure and fresh idle status prove the stop.
  const discard = { request: "Discard the stopped provider failure" };
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
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).discard, {
    ...discard,
    basis: "terminal_failure",
    stopEvidence: "confirmed_failure",
  });
  equal(h.resources.get("state"), original.before);
  assert(!h.child.has("result"));
  await assertNoPendingAgent(h.context);
  h.controls.agentRunner = undefined;
  await h.methods.run_agent.execute({ ...options, role: "triage" }, h.context);
  equal((h.resources.get("state") as LifecycleState).phase, "classified");
});

/** confirm_stopped's refusal when it cannot observe the worker at all. */
const UNOBSERVABLE_STOP =
  "Cannot list panes in session development; the behavior cannot observe whether the completed worker stopped";
/** confirm_stopped's refusal when it observed the worker running. */
const BUSY_STOP =
  'Completed worker is not stopped: agent state is "busy", expected idle';

/** A completed worker whose result was not applied: the lost reply leaves the task running. */
async function completedUnapplied() {
  const h = harness();
  h.controls.runError = "Lost provider response";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Lost provider response",
  );
  const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(original.status, "running");
  return { h, original };
}

Deno.test("discard of a completed worker without attestation freshly observes it", async () => {
  {
    const { h, original } = await completedUnapplied();
    const submission = () =>
      h.submissions.get(original.submission!.behaviorName)?.get("submission");
    const submissionBefore = structuredClone(submission());
    const childBefore = structuredClone(h.child.get("state"));
    const discard = { request: "Discard the unusable completed triage" };
    const calls = h.calls.length;

    // An unobservable or running worker is refused and nothing durable changes.
    h.controls.stopError = UNOBSERVABLE_STOP;
    await rejects(
      () => h.methods.discard_agent.execute(discard, h.context),
      "could not verify",
    );
    h.controls.stopError = BUSY_STOP;
    await rejects(
      () => h.methods.discard_agent.execute(discard, h.context),
      'agent state is "busy"',
    );
    equal(h.calls.slice(calls).map((call) => call.method), [
      "confirm_stopped",
      "confirm_stopped",
    ]);
    equal(h.calls.at(-1)?.definition, original.behaviorName);
    equal(h.resources.get("agent-task"), original);
    equal(await loadState(h.context), original.before);
    equal(h.child.get("state"), childBefore);
    equal(submission(), submissionBefore);
    await rejects(() => assertNoPendingAgent(h.context), "unresolved");

    // A recorded observation other than the pane's fresh idle status is
    // not stop evidence either.
    h.controls.stopError = undefined;
    h.controls.stopOutcome = "pane_closed";
    await rejects(
      () => h.methods.discard_agent.execute(discard, h.context),
      "the stop observation is not a fresh idle status of terminal_7",
    );
    equal(h.resources.get("agent-task"), original);

    h.controls.stopOutcome = undefined;
    await h.methods.discard_agent.execute(discard, h.context);
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(task.status, "discarded");
    equal(task.discard, {
      ...discard,
      basis: "completed_unusable",
      stopEvidence: "fresh_idle",
    });
    equal(
      h.calls.slice(calls).filter((call) => call.method === "confirm_stopped")
        .length,
      4,
    );
    equal(await loadState(h.context), original.before);
    await assertNoPendingAgent(h.context);
  }
});

Deno.test("confirmedStopped stands in only when the behavior cannot observe the completed worker", async () => {
  const discard = {
    request: "Discard the completed triage; I checked the pane has stopped",
    confirmedStopped: true as const,
  };
  // A worker observed running is refused even with the attestation.
  const running = await completedUnapplied();
  const before = running.h.calls.length;
  running.h.controls.stopError = BUSY_STOP;
  await rejects(
    () => running.h.methods.discard_agent.execute(discard, running.h.context),
    "confirmedStopped cannot override an observation",
  );
  equal(running.h.calls.slice(before).map((call) => call.method), [
    "confirm_stopped",
  ]);
  equal(running.h.resources.get("agent-task"), running.original);

  // An observable worker is observed; the attestation is not the evidence.
  const observable = await completedUnapplied();
  await observable.h.methods.discard_agent.execute(
    discard,
    observable.h.context,
  );
  equal(
    AgentTaskSchema.parse(observable.h.resources.get("agent-task")).discard,
    { ...discard, basis: "completed_unusable", stopEvidence: "fresh_idle" },
  );

  // Only an impossible observation (failed or unrecorded) accepts it.
  for (const unrecorded of [false, true]) {
    const { h } = await completedUnapplied();
    const calls = h.calls.length;
    if (unrecorded) {
      const run = h.context.runModel;
      h.context.runModel = (request) =>
        request.method === "confirm_stopped"
          ? Promise.resolve({ ok: true } as Awaited<ReturnType<typeof run>>)
          : run(request);
      h.child.delete("observation");
    } else {
      h.controls.stopError = UNOBSERVABLE_STOP;
    }
    await h.methods.discard_agent.execute(discard, h.context);
    if (!unrecorded) {
      equal(h.calls.slice(calls).map((call) => call.method), [
        "confirm_stopped",
      ]);
    }
    equal(AgentTaskSchema.parse(h.resources.get("agent-task")).discard, {
      ...discard,
      basis: "completed_unusable",
      stopEvidence: "attested",
    });
  }
});

/** A real completed child whose evidence was never submitted. */
async function realCompletedChild() {
  const h = harness();
  const calls: string[][] = [];
  h.controls.agentRunner = recoveryRunner(false, calls);
  h.controls.submitEvidence = false;
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "",
  );
  const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(original.status, "running");
  const childBefore = structuredClone(h.child.get("state"));
  equal(childBefore?.phase, "completed");
  return { h, calls, original, childBefore };
}

Deno.test("discard of a real completed child whose pane is absent requires the operator's attestation", async () => {
  const { h, calls, original, childBefore } = await realCompletedChild();
  const observationBefore = structuredClone(h.child.get("observation"));
  const listings = h.controls.placementCalls.length;
  // The fixture's session lists only the controller pane. The worker may have
  // warped to another session or outlived its server, so absence is not
  // evidence that it stopped.
  const request = "Discard the completed triage without evidence";
  await rejects(
    () => h.methods.discard_agent.execute({ request }, h.context),
    "Pane terminal_7 is not listed in session",
  );
  let message = "";
  try {
    await h.methods.discard_agent.execute({ request }, h.context);
  } catch (error) {
    message = String(error);
  }
  assert(message.includes("Confirm the stop yourself"), message);
  assert(message.includes("confirmedStopped: true"), message);
  equal(h.controls.placementCalls.length, listings + 2);
  equal(h.resources.get("agent-task"), original);
  equal(h.child.get("state"), childBefore);
  // The refusals write no observation.
  equal(h.child.get("observation"), observationBefore);
  await rejects(() => assertNoPendingAgent(h.context), "unresolved");

  // With the attestation the absent pane is discarded as attested.
  await h.methods.discard_agent.execute(
    { request, confirmedStopped: true },
    h.context,
  );
  equal(h.controls.placementCalls.length, listings + 3);
  equal(h.child.get("state"), childBefore);
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).discard, {
    request,
    confirmedStopped: true,
    basis: "completed_unusable",
    stopEvidence: "attested",
  });
  equal(calls.filter((args) => args[3] === "prompt").length, 1);
  await assertNoPendingAgent(h.context);
});

Deno.test("discard of a real completed child observed idle needs no attestation", async () => {
  const { h, calls, childBefore } = await realCompletedChild();
  h.controls.placementFailures.push({
    code: 0,
    stdout: JSON.stringify([
      { id: 1, is_plugin: false, tab_id: h.controls.controlTab },
      { id: 7, is_plugin: false, tab_id: 0 },
    ]),
    stderr: "",
  });
  const request = "Discard the idle completed triage";
  await h.methods.discard_agent.execute({ request }, h.context);
  equal(h.child.get("state"), childBefore);
  equal(h.child.get("observation")?.outcome, "ok");
  equal(h.child.get("observation")?.paneId, "terminal_7");
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).discard, {
    request,
    basis: "completed_unusable",
    stopEvidence: "fresh_idle",
  });
  const status = calls.at(-1)!;
  equal(status.slice(2, 5), ["agent", "status", "terminal_7"]);
  equal(calls.filter((args) => args[3] === "prompt").length, 1);
  await assertNoPendingAgent(h.context);
});

Deno.test("discard rejection names every failed condition", async () => {
  const h = harness();
  h.controls.childPhase = "waiting";
  h.controls.childOutcome = "timed_out";
  h.controls.childPatch = {
    rawResponse: JSON.stringify({
      outcome: "timed_out",
      submission_id: "submission-1",
      acknowledged: true,
      agents: [{
        pane_id: "terminal_7",
        session: options.session,
        agent: { kind: "claude", session_id: "conversation-1", state: "busy" },
        ready: false,
        status_available: true,
        prompt_supported: true,
      }],
    }),
  };
  h.controls.runError = "Provider timed out";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Provider timed out",
  );
  const original = AgentTaskSchema.parse(h.resources.get("agent-task"));
  for (const confirmedStopped of [undefined, true as const]) {
    let message = "";
    try {
      await h.methods.discard_agent.execute({
        request: "Discard",
        ...(confirmedStopped ? { confirmedStopped } : {}),
      }, h.context);
    } catch (error) {
      message = String(error);
    }
    assert(message.includes("uncertain or active"), message);
    assert(message.includes(`task ${original.id} (triage`), message);
    assert(
      message.includes(
        'Completed worker: phase is "waiting", expected completed or completed_unusable; outcome is "timed_out", expected completed',
      ),
      message,
    );
    assert(
      message.includes('Terminal failure: latest response outcome "timed_out"'),
      message,
    );
    assert(
      message.includes(
        'Rejected launch: no version-two launch receipt and phase is "waiting", not launch_rejected',
      ),
      message,
    );
    assert(message.includes("restart-agent"), message);
  }
  // Neither confirm_failure nor confirm_stopped was attempted.
  equal(h.calls.length, 1);
  equal(h.resources.get("agent-task"), original);
  await rejects(() => assertNoPendingAgent(h.context), "unresolved");
});

Deno.test("discard state mismatch names changed lifecycle fields", async () => {
  const { h, original } = await completedUnapplied();
  h.resources.set("state", transition(original.before, "triage", triage));
  const calls = h.calls.length;
  let message = "";
  try {
    await h.methods.discard_agent.execute({
      request: "Discard the completed triage",
      confirmedStopped: true,
    }, h.context);
  } catch (error) {
    message = String(error);
  }
  assert(message.includes("may already be applied"), message);
  assert(message.includes(`task ${original.id}`), message);
  const fields = message.match(/was dispatched \(([^)]*)\)/)?.[1].split(", ") ??
    [];
  assert(fields.includes("phase"), message);
  // Unchanged fields are not named.
  assert(!fields.includes("issue") && !fields.includes("planVersion"), message);
  // Only the read-only stop observation ran before the state check.
  equal(h.calls.slice(calls).map((call) => call.method), ["confirm_stopped"]);
  equal(h.resources.get("agent-task"), original);
});

Deno.test("legacy discard records and caller-supplied basis", async () => {
  const { h, original } = await completedUnapplied();
  const legacy = {
    ...original,
    status: "discarded",
    discard: { request: "Discard", confirmedStopped: true },
  };
  const parsed = AgentTaskSchema.parse(legacy);
  equal(parsed.status, "discarded");
  equal(parsed.discard, legacy.discard);
  const recovery = {
    id: crypto.randomUUID(),
    step: "triage",
    request: "Restart investigation",
    createdAt: timestamp,
    status: "completed",
    before: original.before,
    next: original.before,
    task: legacy,
    replacement: legacy,
  };
  const parsedRecovery = RecoverySchema.parse(recovery);
  equal(parsedRecovery.task?.discard, legacy.discard);
  equal(parsedRecovery.replacement?.discard, legacy.discard);
  equal(
    DriverSchema.parse({ implementation: legacy }).implementation?.discard,
    legacy.discard,
  );
  for (
    const invalid of [
      { request: "Discard", confirmedStopped: false },
      { request: "Discard", basis: "invented" },
      { request: "Discard", stopEvidence: "attested", extra: true },
    ]
  ) {
    assert(
      !AgentTaskSchema.safeParse({ ...original, discard: invalid }).success,
    );
  }

  const supplied = {
    request: "Discard the completed triage; I checked the pane has stopped",
    confirmedStopped: true as const,
    basis: "terminal_failure",
    stopEvidence: "confirmed_failure",
  };
  equal(h.methods.discard_agent.arguments.parse(supplied), {
    request: supplied.request,
    confirmedStopped: true,
  });
  h.controls.stopError = UNOBSERVABLE_STOP;
  await h.methods.discard_agent.execute(supplied, h.context);
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).discard, {
    request: supplied.request,
    confirmedStopped: true,
    basis: "completed_unusable",
    stopEvidence: "attested",
  });
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
  equal(AgentTaskSchema.parse(h.resources.get("agent-task")).discard, {
    ...discard,
    basis: "terminal_failure",
    stopEvidence: "confirmed_failure",
  });
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

    const contract = JSON.parse(assignmentText(task, "schema"));
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
  equal(h.controls.notifications.length, 2);
  equal(h.controls.notifications[0], h.controls.notifications[1]);
  equal(h.controls.notifications[0].slice(0, 6), [
    "--session",
    "development",
    "action",
    "notify",
    "--pane-id",
    "terminal_1",
  ]);
  const control = { session: "development", paneId: "terminal_99" };
  await h.drive.execute({ control }, h.context);
  equal(h.controls.notifications.at(-1)?.[5], control.paneId);
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
  assert(
    workerMaterial(taskOfCall(h, h.calls[3])).includes("Cover the caller"),
  );
  const authorization = DriverSchema.parse(h.resources.get("driver"))
    .revisionAuthorization!;
  equal(authorization.request, approval.request);
  assert(authorization.assignment.prompt.includes(approval.request));
  assert(
    workerMaterial(authorization.assignment).includes(
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
    // A lost response or write after accepted evidence and a durable child
    // completion is re-observed once within the same drive, not relaunched.
    const recovered = fault !== "dispatch";
    equal(
      result(h).decision,
      recovered ? "authorize_plan_revision" : "blocked",
    );
    // A persistent lost response affects both the revision and its review.
    equal(
      h.controls.sleeps,
      fault === "worker" ? [1000, 1000] : recovered ? [1000] : [],
    );
    const authorization = DriverSchema.parse(h.resources.get("driver"))
      .revisionAuthorization!;
    h.controls.runError = undefined;
    // An input retry reuses the recorded authorization.
    if (!recovered) {
      await h.drive.execute({ revisionApproval: approval }, h.context);
    }
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

Deno.test("driver requires legacy worktree preparation after explicit approval, then implements and reviews to merge authorization", async () => {
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

Deno.test("driver asks about code findings before UX review runs and before running checks", async () => {
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
  equal((await loadState(h.context)).uxReview, undefined);
  assert(!dispatchedRoles(h).includes("review_ux"));
  assert(
    result(h).presentation.includes(
      "UX review runs once the code review's findings are settled.",
    ),
  );
  equal(
    result(h).codeReviewFingerprint,
    codeReviewFingerprint(
      await loadState(h.context),
    ),
  );
  const handoffId = result(h).id;
  const calls = h.calls.length;
  await h.drive.execute({}, h.context);
  equal(result(h).id, handoffId);
  equal(h.calls.length, calls);
  equal(h.controls.checkCalls.length, 0);
});

Deno.test("low-severity findings start no round and are listed as follow-ups at merge authorization", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = (task) =>
    task.role === "review_code"
      ? {
        ...driverEvidence(task) as object,
        findings: [{
          id: "nit",
          severity: "low",
          description: "Helper name is unclear",
          affectedPaths: ["lib.rs"],
          evidence: ["lib.rs:4"],
        }],
      }
      : driverEvidence(task);
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "authorize_merge");
  assert(dispatchedRoles(h).includes("review_ux"));
  assert(!dispatchedRoles(h).includes("implement_review_fixes"));
  for (
    const text of [
      "Follow-ups (low severity, not repaired; select one of the current reviews with --repair to fix it now):",
      "- review_code/nit: Helper name is unclear",
    ]
  ) assert(result(h).presentation.includes(text), text);
  equal(
    (await loadState(h.context)).reviewFollowUps!.map((f) =>
      f.finding.findingId
    ),
    ["nit"],
  );
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

const formatBaseline = {
  id: "format",
  command: ["cargo", "x", "format", "--check"],
};
const lintBaseline = { id: "lint", command: ["cargo", "x", "lint"] };

Deno.test("drive runs the rebase probe and baseline checks before plan checks", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  h.controls.verificationSettings = () =>
    Promise.resolve({
      baselineChecks: [formatBaseline, lintBaseline],
      rebaseProbe: true,
    });
  await h.drive.execute({ agent: options }, h.context);
  const handoff = result(h);
  equal(handoff.decision, "authorize_merge");
  const verification = (await loadState(h.context)).verification!;
  equal(verification.status, "passed");
  equal(
    h.controls.gitActions.filter((a) => a === "rebase-probe" || a === "check"),
    ["rebase-probe", "check", "check", "check", "check"],
  );
  equal(
    h.controls.checkCalls.map(({ request, log, spec }) => [
      request.argv,
      log,
      spec,
    ]),
    [
      [formatBaseline.command, `baseline-log-${verification.id}-format`],
      [lintBaseline.command, `baseline-log-${verification.id}-lint`],
      ...driverPlan.checks.map((check) => [
        check.command,
        `check-log-${verification.id}-${check.id}`,
      ]),
    ].map(([argv, log]) => [argv, log, "check-log"]),
  );
  equal(verification.rebaseProbe, { main: "b".repeat(40), status: "clean" });
  equal(
    verification.baseline?.map((check) => [check.id, check.status, check.log]),
    [
      ["format", "passed", `baseline-log-${verification.id}-format`],
      ["lint", "passed", `baseline-log-${verification.id}-lint`],
    ],
  );
  equal(verification.checks.map((check) => check.id), [
    "regression",
    "caller",
  ]);
  const lines = handoff.presentation.split("\n");
  const index = (prefix: string) =>
    lines.findIndex((line) => line.startsWith(prefix));
  const order = [
    `- [x] rebase probe: no conflicts with local main ${"b".repeat(40)}`,
    "- [x] baseline format: cargo x format --check -> exit 0 in 1.2s. Log: swamp data query",
    "- [x] baseline lint: cargo x lint -> exit 0 in 1.2s. Log:",
    "- [x] regression:",
  ].map(index);
  assert(
    order.every((i, n) => i >= 0 && (n === 0 || i > order[n - 1])),
    handoff.presentation,
  );
  assert(
    !lines[order[1]].includes("covers"),
    "Baseline checks cover no plan step",
  );
});

Deno.test("failed baseline check or rebase conflict blocks drive with repair guidance", async () => {
  for (const kind of ["baseline", "conflicts"] as const) {
    const h = harness(driverImplementing());
    h.controls.evidenceForRole = driverEvidence;
    h.controls.verificationSettings = () =>
      Promise.resolve({ baselineChecks: [lintBaseline], rebaseProbe: true });
    if (kind === "baseline") {
      h.controls.checkOutcome = (request) => ({
        exitCode: (request.argv as string[]).includes("lint") ? 101 : 0,
        durationMs: 50,
        worktreeUnchanged: true,
      });
    } else {
      h.controls.rebaseProbe = () => ({
        main: "b".repeat(40),
        status: "conflicts",
        paths: ["src/lib.rs", "docs/a b.md"],
      });
    }
    await h.drive.execute({ agent: options }, h.context);
    const handoff = result(h);
    equal(handoff.decision, "blocked");
    const verification = (await loadState(h.context)).verification!;
    equal(verification.status, "failed");
    // Every check still ran, so the checklist is complete.
    equal(h.controls.checkCalls.length, 3);
    assert(
      handoff.reason.startsWith(
        `Verification ${verification.id} failed at ${head}: ${
          kind === "baseline"
            ? `baseline lint (exit 101; log: swamp data query 'modelName == "issue-default-85" && name == "baseline-log-${verification.id}-lint"' --select content)`
            : `rebase conflicts with local main ${
              "b".repeat(40)
            } in src/lib.rs, docs/a b.md`
        }. Run repair (no input needed: the recorded failures become lastFailure for the implementation worker; add a reason only for context the logs lack), or rerun verify explicitly for an environment failure at this same commit`,
      ),
      handoff.reason,
    );
    assert(!handoff.reason.includes("regression ("), handoff.reason);
    // Only a baseline failure can come from the branch's older base.
    equal(
      handoff.reason.includes(
        "a baseline failure in code this branch did not change can come from its older base",
      ),
      kind === "baseline",
    );
    assert(
      handoff.presentation.includes(
        kind === "baseline"
          ? "- [ ] baseline lint: cargo x lint -> exit 101"
          : `- [ ] rebase probe: conflicts with local main ${
            "b".repeat(40)
          } in src/lib.rs, docs/a b.md`,
      ),
      handoff.presentation,
    );
    // Drive never repairs or reruns verification on its own.
    await h.drive.execute({}, h.context);
    equal(result(h).decision, "blocked");
    equal(h.controls.checkCalls.length, 3);
    equal((await loadState(h.context)).verification?.id, verification.id);
  }
});

Deno.test("an unavailable rebase probe is recorded without failing verification", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  h.controls.verificationSettings = () =>
    Promise.resolve({ baselineChecks: [], rebaseProbe: true });
  h.controls.rebaseProbe = () => {
    throw new GitOperationError("expectedHead is not a commit", {});
  };
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "authorize_merge");
  const verification = (await loadState(h.context)).verification!;
  equal(verification.status, "passed");
  equal(verification.baseline, undefined);
  equal(verification.rebaseProbe, {
    status: "unavailable",
    reason: "expectedHead is not a commit",
  });
  assert(
    result(h).presentation.includes(
      "- rebase probe unavailable: expectedHead is not a commit (not a failure; the merge gate rebases under its lock)",
    ),
    result(h).presentation,
  );
});

Deno.test("verification settings errors stop drive before any check runs", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  const message =
    "/repo/control/.yinz/issue-lifecycle.json: verification.baselineChecks[0].id must match [a-z0-9][a-z0-9_-]{0,47}";
  h.controls.verificationSettings = () => Promise.reject(new Error(message));
  await h.drive.execute({ agent: options }, h.context);
  const state = await loadState(h.context);
  equal(state.verification, undefined);
  assert(state.uxReview, "Both reviews completed before verification");
  equal(h.controls.checkCalls.length, 0);
  assert(!h.controls.gitActions.includes("rebase-probe"));
  equal(result(h).decision, "blocked");
  equal(result(h).reason, message);
});

Deno.test("repair of a failed verification hands its logs to the implementation worker and records the round cause", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  h.controls.verificationSettings = () =>
    Promise.resolve({ baselineChecks: [lintBaseline], rebaseProbe: false });
  h.controls.checkOutcome = (request) => ({
    exitCode: (request.argv as string[]).includes("lint") ? 101 : 0,
    durationMs: 50,
    worktreeUnchanged: true,
  });
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  const implementPrompts = () =>
    h.writes.filter((w) =>
      w.spec === "agent-task" && w.value.role === "implement"
    ).map((w) => AgentTaskSchema.parse(w.value));
  const first = implementPrompts().at(-1)!;
  assert(!first.prompt.includes("Read each log named in lastFailure"));
  const failed = await loadState(h.context);
  const verification = failed.verification!;
  // No reason is needed: the recorded failure becomes lastFailure.
  await saveState(h.context, "repair", transition(failed, "repair", {}));
  const repaired = await loadState(h.context);
  assert(
    repaired.lastFailure?.startsWith(
      `Verification ${verification.id} failed at ${head} (plan version 1): baseline lint (exit 101; log: baseline-log-${verification.id}-lint)`,
    ),
    repaired.lastFailure,
  );
  await h.drive.execute({}, h.context);
  const task = implementPrompts().find((t) => t.id !== first.id)!;
  assert(task, "drive dispatched a new implementation worker");
  assert(
    workerMaterial(task).includes(repaired.lastFailure!),
    "lastFailure in the bound packet",
  );
  assert(
    task.prompt.includes(
      `Read each log named in lastFailure (read-only; this is not the evidence interface) with: env -u SWAMP_SERVE_URL -u SWAMP_SERVER_URL -u SWAMP_REPO_DIR -u SWAMP_WORKFLOWS_DIR swamp data query 'modelName == "issue-default-85" && name == "<log>"' --select content --repo-dir '/repo/control'`,
    ),
    task.prompt,
  );
  const rounds = h.resources.get("rounds") as {
    records: {
      assignmentId: string;
      launchCauses: { kind: string; explanation: string }[];
    }[];
  };
  const round = rounds.records.find((r) => r.assignmentId === task.id)!;
  const cause = round.launchCauses.find((c) =>
    c.explanation === repaired.lastFailure
  );
  equal(cause?.kind, "verification_merge_failure");
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
  const logs: string[] = [];
  h.context.logger = {
    info: (message, values) =>
      logs.push(`${message} ${JSON.stringify(values)}`),
  };
  await h.drive.execute({ agent: options }, h.context);
  // The applying task is re-observed once from its accepted evidence.
  assert(logs.some((line) => line.includes("reobserve")));
  equal(h.controls.sleeps, [1000]);
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
  // Before any approval, drive refreshes a changed Agenda and plans again.
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  h.controls.agenda = { ...issue, digest: "e".repeat(64) };
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_plan");
  equal(h.calls.length, 6);
  assert(refreshEvents(h).length === 1);
});

function refreshEvents(h: ReturnType<typeof harness>) {
  return h.writes.filter((write) =>
    write.spec === "event" && write.name.startsWith("event-refresh-")
  );
}

Deno.test("driver refreshes changed requirements before approval and re-plans", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  const approval = result(h);
  h.controls.agenda = { ...issue, digest: "e".repeat(64), note: "Changed" };
  await h.drive.execute({}, h.context);
  const events = refreshEvents(h);
  equal(events.map((event) => event.name), [
    `event-refresh-${"d".repeat(16)}-${"e".repeat(16)}-g1`,
  ]);
  equal(events[0].value.action, "refresh");
  equal((events[0].value.state as LifecycleState).phase, "triaging");
  const state = await loadState(h.context);
  equal(state.issue, h.controls.agenda);
  equal(state.planningContext!.generation, 1);
  // The previous generation's reviewed plan is history, not current evidence.
  const previous = state.planningContext!.history.filter((entry) =>
    entry.generation === 0
  );
  equal(previous.map((entry) => entry.plan.version), [1]);
  assert(previous[0].review);
  equal(dispatchedRoles(h), [
    "triage",
    "plan",
    "review_plan",
    "triage",
    "plan",
    "review_plan",
  ]);
  equal(result(h).decision, "approve_plan");
  equal(result(h).planVersion, 2);
  // The earlier approval handoff can no longer be used.
  assert(result(h).id !== approval.id);
  const calls = h.calls.length;
  await h.drive.execute({}, h.context);
  equal(refreshEvents(h).length, 1);
  equal(h.calls.length, calls);
  equal(result(h).decision, "approve_plan");

  // A crash after the event write but before the state write replays the event.
  const retry = harness();
  retry.controls.evidenceForRole = driverEvidence;
  await retry.drive.execute({ agent: options }, retry.context);
  retry.controls.agenda = h.controls.agenda;
  retry.controls.failWrite = (spec, _name, value) =>
    spec === "state" && value.phase === "triaging";
  await retry.drive.execute({}, retry.context);
  equal(result(retry).decision, "blocked");
  equal(refreshEvents(retry).length, 1);
  equal((await loadState(retry.context)).issue.digest, issue.digest);
  await retry.drive.execute({}, retry.context);
  equal(result(retry).decision, "approve_plan");
  equal(refreshEvents(retry).length, 1);
  equal(result(retry).planVersion, 2);

  // Requirements that change back and forth keep distinct refresh events.
  h.controls.agenda = issue;
  await h.drive.execute({}, h.context);
  h.controls.agenda = { ...issue, digest: "e".repeat(64), note: "Changed" };
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_plan");
  equal(refreshEvents(h).map((event) => event.name.slice(-3)), [
    "-g1",
    "-g2",
    "-g3",
  ]);
});

for (const phase of ["triaging", "classified"] as const) {
  Deno.test(`driver refreshes changed requirements while ${phase} before dispatching`, async () => {
    const initial = phase === "triaging"
      ? createState(issue)
      : transition(createState(issue), "triage", triage);
    const h = harness(initial);
    h.controls.evidenceForRole = driverEvidence;
    h.controls.agenda = { ...issue, digest: "e".repeat(64), note: "Changed" };
    await h.drive.execute({ agent: options }, h.context);
    equal(refreshEvents(h).length, 1);
    equal(dispatchedRoles(h), ["triage", "plan", "review_plan"]);
    equal(result(h).decision, "approve_plan");
    equal((await loadState(h.context)).issue, h.controls.agenda);
  });
}

Deno.test("driver blocks changed requirements after approval with the change and the approvals refresh would invalidate", async () => {
  const h = harness(implementing());
  h.controls.evidenceForRole = driverEvidence;
  h.controls.agenda = { ...issue, digest: "e".repeat(64), note: "Changed" };
  const before = await loadState(h.context);
  await h.drive.execute({ agent: options }, h.context);
  const handoff = result(h);
  equal(handoff.decision, "blocked");
  assert(handoff.reason.includes("requirements changed after plan approval"));
  equal(handoff.requirementsChange, {
    previousDigest: issue.digest,
    digest: "e".repeat(64),
    changes: ["- Acceptance criteria", "+ Changed"],
    invalidates: ['plan approval for version 1: "Implement this plan"'],
  });
  assert(
    handoff.presentation.includes(
      "Requirements changes:\n  - Acceptance criteria\n  + Changed",
    ),
  );
  assert(
    handoff.presentation.includes(
      'Refresh would invalidate:\n- plan approval for version 1: "Implement this plan"',
    ),
  );
  equal(await loadState(h.context), before);
  equal(refreshEvents(h).length, 0);
  equal(h.calls.length, 0);
});

Deno.test("driver will not auto-refresh changed requirements over an unspent revision authorization", async () => {
  const h = harness();
  h.controls.evidenceForRole = blockedPlanEvidence;
  await h.drive.execute({ agent: options }, h.context);
  const approval = revisionApproval(h);
  h.controls.failWrite = (spec, _name, value) =>
    spec === "agent-task" && value.status === "running";
  await h.drive.execute({ revisionApproval: approval }, h.context);
  equal(result(h).decision, "blocked");
  const calls = h.calls.length;
  const before = await loadState(h.context);
  h.controls.agenda = {
    ...issue,
    digest: "e".repeat(64),
    note: "Changed requirements",
  };
  await h.drive.execute({}, h.context);
  const handoff = result(h);
  equal(handoff.decision, "blocked");
  assert(
    handoff.reason.includes(
      `revision authorization for handoff ${approval.handoffId}`,
    ),
  );
  equal(handoff.requirementsChange!.invalidates, []);
  equal(refreshEvents(h).length, 0);
  equal(await loadState(h.context), before);
  equal(h.calls.length, calls);
  // refresh's precondition refuses while the authorization is reserved, so the
  // reason must name the explicit rollback instead.
  await rejects(
    () => assertNoPendingAgent(h.context),
    "Reserved pane assignment is unresolved",
  );
  assert(!handoff.reason.includes("run refresh"), handoff.reason);
  assert(handoff.reason.includes("rollback to triage"), handoff.reason);
  await h.methods.rollback.execute({
    step: "triage",
    request: "Re-plan against the changed requirements",
  }, h.context);
  const rolledBack = await loadState(h.context);
  equal(rolledBack.phase, "triaging");
  equal(rolledBack.issue.digest, "e".repeat(64));
  assert(
    rolledBack.planningContext!.userFeedback.some((f) =>
      f.request === approval.request
    ),
  );
  await assertNoPendingAgent(h.context);
  await h.drive.execute({}, h.context);
  assert(
    !result(h).reason?.includes("Agenda requirements changed"),
    result(h).reason,
  );
  assert(h.calls.length > calls);
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
      "unconsumed-input",
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
              not_ready_reason: scenario === "unconsumed-input"
                ? "unconsumed_input"
                : "unresolved_turn",
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
    // Ordinary timeouts re-observe immediately; nothing else is retried.
    equal(h.controls.sleeps, []);
    if (
      ["timeouts", "timeout-detail", "unconsumed-input", "startup"].includes(
        scenario,
      )
    ) {
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
      assert(
        result(h).reason.includes("Automatic recovery not applicable"),
        result(h).reason,
      );
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

function idleAgent(task: ReturnType<typeof AgentTaskSchema.parse>) {
  return {
    pane_id: "terminal_7",
    session: options.session,
    agent: { kind: "claude", session_id: "conversation-1", state: "idle" },
    cwd: task.config.cwd,
    ready: true,
    status_available: true,
    prompt_supported: true,
  };
}
function busyAgent(
  task: ReturnType<typeof AgentTaskSchema.parse>,
  reason = "unconsumed_input",
) {
  const agent = idleAgent(task);
  return {
    ...agent,
    agent: { ...agent.agent, state: "Busy" },
    ready: false,
    not_ready_reason: reason,
  };
}
function reply(value: Record<string, unknown>) {
  return Promise.resolve({
    code: 0,
    stdout: JSON.stringify(value),
    stderr: "",
  });
}
function recoveryLogs(h: ReturnType<typeof harness>) {
  const logs: string[] = [];
  h.context.logger = {
    info: (message, values) =>
      logs.push(`${message} ${JSON.stringify(values)}`),
  };
  return logs;
}
function triageTask(h: ReturnType<typeof harness>) {
  return AgentTaskSchema.parse(
    h.writes.find((write) =>
      write.spec === "agent-task" && write.value.role === "triage"
    )!.value,
  );
}

Deno.test("driver resumes a lost prompt response from the submitting phase on its original submission", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  const abort = new AbortController();
  h.context.signal = abort.signal;
  const cli: string[][] = [];
  let lost = true;
  let timedOut = false;
  h.controls.agentRunner = async (args) => {
    cli.push(args);
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    if (args[3] === "spawn") {
      return reply({
        outcome: "ok",
        spawned: { pane_id: "terminal_7", cwd: task.config.cwd },
      });
    }
    if (args.includes("--until")) {
      return reply({ outcome: "ok", agents: [idleAgent(task)] });
    }
    if (args[3] === "prompt" && lost) {
      // The worker accepted the prompt, but its response never arrived.
      lost = false;
      await h.submitEvidence();
      abort.abort(new Error("Controller lost the prompt response"));
      throw new Error("Prompt response lost");
    }
    if (args[3] === "wait" && task.role === "triage" && !timedOut) {
      timedOut = true;
      return reply({
        outcome: "timed_out",
        submission_id: "submission-1",
        acknowledged: true,
        prompt_delivery: "started",
        agents: [busyAgent(task)],
      });
    }
    return reply({
      outcome: "completed",
      submission_id: "submission-1",
      acknowledged: true,
      prompt_delivery: "started",
      agents: [idleAgent(task)],
    });
  };
  await rejects(
    () => h.drive.execute({ agent: options }, h.context),
    "Controller lost the prompt response",
  );
  equal(AgentStateSchema.parse(h.child.get("state")).phase, "submitting");
  h.context.signal = new AbortController().signal;
  const logs = recoveryLogs(h);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_plan");
  assert(timedOut);
  assert(logs.some((line) => line.includes('"class":"long_wait"')));
  equal(h.controls.sleeps, []);
  equal(cli.filter((args) => args[3] === "spawn").length, 3);
  equal(cli.filter((args) => args[3] === "prompt").length, 3);
  const triage = triageTask(h);
  equal(
    h.calls.filter((call) => call.name === triage.behaviorName)
      .map((call) => call.method),
    ["run", "recover", "recover"],
  );
});

Deno.test("driver keeps observing background work while unconsumed input is reported", async () => {
  for (const timeoutRounds of [1, 2]) {
    const h = harness();
    h.controls.evidenceForRole = driverEvidence;
    const cli: string[][] = [];
    let backgroundWaits = 0;
    let prompted = false;
    h.controls.agentRunner = (args) => {
      cli.push(args);
      const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
      if (args[3] === "spawn") {
        return reply({
          outcome: "ok",
          spawned: { pane_id: "terminal_7", cwd: task.config.cwd },
        });
      }
      if (args.includes("--until")) {
        if (task.role === "triage" && prompted) {
          backgroundWaits++;
          if (backgroundWaits <= timeoutRounds) {
            return reply({ outcome: "timed_out", agents: [busyAgent(task)] });
          }
        }
        return reply({ outcome: "ok", agents: [idleAgent(task)] });
      }
      prompted = true;
      return reply({
        outcome: "completed",
        submission_id: "submission-1",
        acknowledged: true,
        prompt_delivery: "started",
        agents: [idleAgent(task)],
        ...(task.role === "triage" ? { background_tasks: ["worker"] } : {}),
      });
    };
    await h.drive.execute({ agent: options }, h.context);
    if (timeoutRounds === 2) {
      equal(result(h).decision, "blocked");
      equal(
        DriverSchema.parse(h.resources.get("driver")).backgroundStall?.child
          .phase,
        "background",
      );
      assert(!result(h).reason.includes("Yinz has not confirmed"));
      await h.methods.resume_agent.execute({}, h.context);
      await h.drive.execute({}, h.context);
    }
    equal(result(h).decision, "approve_plan");
    equal(backgroundWaits, timeoutRounds + 1);
    equal(h.controls.sleeps, []);
    equal(cli.filter((args) => args[3] === "spawn").length, 3);
    equal(cli.filter((args) => args[3] === "prompt").length, 3);
  }
});

Deno.test("driver bounds long waits by the drive deadline", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  const cli: string[][] = [];
  // Longer than the configured window, so the deadline binds before the
  // 48-window attempt limit.
  const window = 900_000;
  h.controls.agentRunner = (args) => {
    cli.push(args);
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    if (args[3] === "spawn") {
      return reply({
        outcome: "ok",
        spawned: { pane_id: "terminal_7", cwd: task.config.cwd },
      });
    }
    if (args.includes("--until")) {
      return reply({ outcome: "ok", agents: [idleAgent(task)] });
    }
    h.controls.clock += window;
    return reply({
      outcome: "timed_out",
      submission_id: "submission-1",
      acknowledged: true,
      prompt_delivery: "started",
      agents: [busyAgent(task, "unresolved_turn")],
    });
  };
  const waits = () =>
    cli.filter((args) => args[3] === "wait" && args.includes("--submission"));
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  assert(
    result(h).reason.includes("long_wait recovery exhausted"),
    result(h).reason,
  );
  assert(result(h).reason.includes("8 h drive deadline"));
  equal(cli.filter((args) => args[3] === "spawn").length, 1);
  equal(cli.filter((args) => args[3] === "prompt").length, 1);
  const first = waits().length;
  equal(first, Math.floor(RECOVERY_DEADLINE_MS / window));
  for (const args of waits()) {
    equal(args[args.indexOf("--submission") + 1], "submission-1");
    equal(args[args.indexOf("--conversation") + 1], "conversation-1");
  }
  equal(h.controls.sleeps, []);
  // A deliberate rerun starts a new budget and keeps waiting on the original.
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  equal(waits().length, first + first + 1);
  equal(cli.filter((args) => args[3] === "prompt").length, 1);
  equal(cli.filter((args) => args[3] === "spawn").length, 1);
});

Deno.test("driver reconciles a lost child response from durable completion without relaunch", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  h.controls.runErrorQueue = ["Swamp child response lost"];
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  const triage = triageTask(h);
  equal(
    h.calls.filter((call) => call.name === triage.behaviorName)
      .map((call) => call.method),
    ["run"],
  );
  equal(h.controls.sleeps, [1000]);
  equal(dispatchedRoles(h), ["triage", "plan", "review_plan"]);
});

Deno.test("driver re-observes a submission wait whose CLI stderr ends in a newline", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  const cli: string[][] = [];
  let failed = false;
  h.controls.agentRunner = (args) => {
    cli.push(args);
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    if (args[3] === "spawn") {
      return reply({
        outcome: "ok",
        spawned: { pane_id: "terminal_7", cwd: task.config.cwd },
      });
    }
    if (args.includes("--until")) {
      return reply({ outcome: "ok", agents: [idleAgent(task)] });
    }
    if (args[3] === "prompt" && task.role === "triage" && !failed) {
      return reply({
        outcome: "timed_out",
        submission_id: "submission-1",
        acknowledged: true,
        prompt_delivery: "started",
        agents: [busyAgent(task, "not_idle")],
      });
    }
    if (args[3] === "wait" && task.role === "triage" && !failed) {
      failed = true;
      // The child stores this trimmed; the controller sees the newline.
      return Promise.resolve({
        code: 1,
        stdout: "",
        stderr: "Cannot resolve control pane: connection lost\n",
      });
    }
    return reply({
      outcome: "completed",
      submission_id: "submission-1",
      acknowledged: true,
      prompt_delivery: "started",
      agents: [idleAgent(task)],
    });
  };
  const logs = recoveryLogs(h);
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  assert(failed);
  assert(
    logs.some((line) => line.includes('"class":"reobserve"')),
    logs.join("\n"),
  );
  equal(cli.filter((args) => args[3] === "spawn").length, 3);
  equal(cli.filter((args) => args[3] === "prompt").length, 3);
  const triage = triageTask(h);
  equal(
    h.calls.filter((call) => call.name === triage.behaviorName)
      .map((call) => call.method)[0],
    "run",
  );
  assert(
    h.calls.filter((call) => call.name === triage.behaviorName)
      .slice(1).every((call) => call.method === "recover"),
  );
});

Deno.test("driver retries transient worker cleanup then proceeds", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  h.controls.cleanupRefusals = [
    {
      outcome: "rejected",
      message: "worker is Busy, not idle",
      retryable: true,
    },
    {
      outcome: "rejected",
      message: "worker background work is still running",
      retryable: true,
    },
  ];
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  // Three attempts for triage, then one each for plan and review_plan.
  equal(h.controls.cleanupCalls, 5);
  equal(h.controls.sleeps, [2000, 5000]);
  const triage = triageTask(h);
  equal(
    h.writes.filter((write) => write.name === `event-agent-${triage.id}`)
      .length,
    1,
  );
  assert(
    h.writes.some((write) =>
      write.spec === "agent-task" && write.value.id === triage.id &&
      write.value.cleanup === "done"
    ),
  );
  equal(h.calls.filter((call) => call.name === triage.behaviorName).length, 1);
});

Deno.test("driver stops cleanup retries at the budget or on correlation refusals", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  h.controls.cleanupRefusals = Array.from({ length: 7 }, () => ({
    outcome: "rejected",
    message: "worker is Busy, not idle",
    retryable: true,
  }));
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  equal(h.controls.cleanupCalls, 7);
  equal(h.controls.sleeps, [2000, 5000, 10000, 20000, 30000, 60000]);
  assert(
    result(h).reason.includes("cleanup recovery exhausted after 6"),
    result(h).reason,
  );
  assert(result(h).reason.includes("only guarded pane cleanup remains"));
  const pending = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal([pending.role, pending.status, pending.cleanup], [
    "triage",
    "completed",
    "pending",
  ]);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_plan");
  equal(h.calls.filter((call) => call.name === pending.behaviorName).length, 1);

  const refused = harness();
  refused.controls.evidenceForRole = driverEvidence;
  refused.controls.cleanupRefusals = [{
    outcome: "rejected",
    message: "submission x is not the worker's latest submission (y)",
    retryable: false,
  }];
  await refused.drive.execute({ agent: options }, refused.context);
  equal(result(refused).decision, "blocked");
  equal(refused.controls.cleanupCalls, 1);
  equal(refused.controls.sleeps, []);
  assert(
    result(refused).reason.includes(
      "Automatic recovery not applicable: cleanup refusal requires inspection",
    ),
    result(refused).reason,
  );
  assert(result(refused).reason.includes("only guarded pane cleanup remains"));
});

Deno.test("driver retries a preflight transport failure before launch intent", async () => {
  const h = observedHarness();
  h.controls.evidenceForRole = driverEvidence;
  for (let role = 0; role < 3; role++) {
    h.turns.push({
      response: {
        outcome: "completed",
        acknowledged: true,
        prompt_delivery: "started",
      },
      submit: true,
    });
  }
  h.controls.placementFailures = [{
    code: 1,
    stdout: "",
    stderr:
      "This session is updating Yinz; retry the action after reconnecting.",
  }];
  const logs = recoveryLogs(h);
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  equal(h.cli.filter(({ args }) => args[3] === "spawn").length, 3);
  equal(h.controls.sleeps, [5000]);
  assert(logs.some((line) => line.includes('"class":"preflight"')));
  const triage = triageTask(h);
  equal(
    h.calls.filter((call) => call.name === triage.behaviorName)
      .map((call) => call.method),
    ["run", "recover"],
  );

  // Missing hooks are not a transport failure.
  const hooks = observedHarness();
  hooks.controls.evidenceForRole = driverEvidence;
  const runner = hooks.controls.agentRunner!;
  hooks.controls.agentRunner = (args, stdin, signal) =>
    args[2] === "hooks"
      ? reply({ status: "not wired" })
      : runner(args, stdin, signal);
  await hooks.drive.execute(
    { agent: { ...options, kind: "codex" } },
    hooks.context,
  );
  equal(result(hooks).decision, "blocked");
  assert(
    result(hooks).reason.includes(
      "Automatic recovery not applicable: hooks_unavailable",
    ),
    result(hooks).reason,
  );
  equal(hooks.controls.sleeps, []);
  equal(hooks.cli.filter(({ args }) => args[3] === "spawn").length, 0);
  equal(hooks.calls.length, 1);
});

Deno.test("drive's prompt observation interruption continues on the submission wait", async () => {
  for (const own of [false, true]) {
    const h = observedHarness();
    h.controls.evidenceForRole = driverEvidence;
    const completed = {
      response: {
        outcome: "completed",
        acknowledged: true,
        prompt_delivery: "started",
      },
      submit: true,
    };
    h.turns.push(
      {
        response: {
          outcome: "interrupted",
          acknowledged: true,
          prompt_delivery: "started",
        },
      },
      own
        ? {
          response: {
            outcome: "interrupted",
            acknowledged: true,
            prompt_delivery: "started",
          },
        }
        : completed,
      completed,
      completed,
    );
    await h.drive.execute({ agent: options }, h.context);
    const prompts = h.cli.filter(({ args }) => args[3] === "prompt");
    const waits = h.cli.filter(({ args }) =>
      args[3] === "wait" && args.includes("--submission")
    );
    equal(h.controls.sleeps, []);
    if (own) {
      // The submission's own interruption is a real turn outcome.
      equal(result(h).decision, "blocked");
      assert(
        result(h).reason.includes(
          "Automatic recovery not applicable: interrupted",
        ),
        result(h).reason,
      );
      equal(prompts.length, 1);
      equal(waits.length, 1);
      equal(h.turns.length, 2);
    } else {
      equal(result(h).decision, "approve_plan");
      equal(prompts.length, 3);
      equal(waits.length, 1);
      equal(dispatchedRoles(h), ["triage", "plan", "review_plan"]);
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
  // Three worker runs plus discard's own confirm_stopped observation.
  equal(h.calls.map((call) => call.method).filter((m) => m !== "run"), [
    "confirm_stopped",
  ]);
  equal(h.calls.length, 5);
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

Deno.test("driver finishes a pending rollback's saved intent and reports an unavailable Agenda", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
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
  const pending = RecoverySchema.parse(h.resources.get("recovery"));
  equal(pending.status, "pending");
  const logs: string[] = [];
  h.context.logger = {
    info: (message, values) =>
      logs.push(`${message} ${JSON.stringify(values)}`),
  };
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  equal(
    result(h).reason,
    `Rollback is incomplete (recovery ${pending.id} to triage): Agenda unavailable. Rerunning drive retries the saved intent; a new explicit rollback may supersede it.`,
  );
  assert(logs.some((line) => line.includes("Restart investigation")));
  equal(h.calls.length, 0);
  equal(
    RecoverySchema.parse(h.resources.get("recovery")).status,
    "pending",
  );
  const reopened = h.controls.reopened;
  h.controls.reopenError = undefined;
  await h.drive.execute({ agent: options }, h.context);
  equal(
    RecoverySchema.parse(h.resources.get("recovery")).status,
    "completed",
  );
  equal(h.controls.reopened, reopened + 1);
  equal(dispatchedRoles(h), ["triage", "plan", "review_plan"]);
  equal(result(h).decision, "approve_plan");
  equal(result(h).recoveryId, pending.id);
});

Deno.test("driver resumes a pending restart replacement exactly once without nesting the issue lock", async () => {
  const h = harness();
  h.controls.evidenceForRole = driverEvidence;
  h.controls.runError = "session disappeared";
  // Without accepted evidence, a lost response is not reconciled automatically.
  h.controls.submitEvidence = false;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  h.controls.submitEvidence = true;
  const staleHandoff = result(h).id;
  const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
  h.controls.reopenError = "Agenda unavailable";
  await rejects(
    () =>
      h.methods.restart_agent.execute({
        ...options,
        role: "triage",
        taskId: old.id,
        request: "Restart triage in a fresh pane",
      }, h.context),
    "Agenda unavailable",
  );
  const pending = RecoverySchema.parse(h.resources.get("recovery"));
  equal(pending.status, "pending");
  const replacement = pending.replacement!;
  equal(h.calls.length, 1);
  await rejects(
    () => assertNoPendingAgent(h.context),
    "run drive or resume-rollback",
  );
  h.controls.reopenError = undefined;
  h.controls.runError = undefined;
  await h.drive.execute({}, h.context);
  equal(
    RecoverySchema.parse(h.resources.get("recovery")).status,
    "completed",
  );
  equal(result(h).decision, "approve_plan");
  equal(h.calls.filter((call) => call.name === old.behaviorName).length, 1);
  equal(
    h.calls.filter((call) => call.name === replacement.behaviorName).length,
    1,
  );
  equal(h.calls.length, 4);
  const calls = h.calls.length;
  await h.drive.execute({
    revisionApproval: { handoffId: staleHandoff, request: "Revise the plan" },
  }, h.context);
  equal(result(h).decision, "blocked");
  assert(result(h).reason.includes("stale"), result(h).reason);
  equal(h.calls.length, calls);
});

Deno.test("driver adopts the recorded worker worktree after approval and continues to implementation", async () => {
  const approved: LifecycleState = {
    ...implementing(),
    phase: "approved",
    workerWorktree: "/repo/issue",
  };
  delete approved.implementation;
  const h = harness(approved);
  h.controls.evidenceForRole = driverEvidence;
  const logs: string[] = [];
  h.context.logger = {
    info: (message, values) =>
      logs.push(`${message} ${JSON.stringify(values)}`),
  };
  await h.drive.execute({ agent: options }, h.context);
  equal(h.controls.prepareCalls, [{
    action: "prepare",
    worktree: "/repo/issue",
    branch: "issue-85",
    issueKey: "default/85",
  }]);
  assert(
    logs.some((line) =>
      line.startsWith("Adopted worker worktree") && line.includes("issue-85")
    ),
  );
  equal(dispatchedRoles(h), ["implement", "review_code", "review_ux"]);
  equal(result(h).decision, "authorize_merge");
  equal((await loadState(h.context)).implementation?.worktree, "/repo/issue");
  equal((await loadState(h.context)).mergeAuthorization, undefined);
});

Deno.test("driver re-adopts the kept implementation after a rollback is re-approved", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "authorize_merge");
  await h.methods.rollback.execute({
    step: "approve",
    request: "Re-approve after reconsidering scope",
  }, h.context);
  const rolledBack = await loadState(h.context);
  equal(rolledBack.implementation?.branch, "issue-85");
  h.resources.set(
    "state",
    transition(rolledBack, "approve", {
      planVersion: rolledBack.plan!.version,
      request: "Implement this plan again",
    }),
  );
  const roles = dispatchedRoles(h).length;
  await h.drive.execute({}, h.context);
  // The kept implementation determines both the worktree and the branch.
  equal(h.controls.prepareCalls, [{
    action: "prepare",
    worktree: "/repo/issue",
    branch: "issue-85",
    issueKey: "default/85",
  }]);
  equal(dispatchedRoles(h).slice(roles), [
    "implement",
    "review_code",
    "review_ux",
  ]);
  equal(result(h).decision, "authorize_merge");
});

Deno.test("driver blocks adoption of a detached or main worker worktree without Git preparation", async () => {
  for (
    const [branch, text] of [["", "at a detached HEAD"], ["main", "on main"]]
  ) {
    const approved: LifecycleState = {
      ...implementing(),
      phase: "approved",
      workerWorktree: "/repo/issue",
    };
    delete approved.implementation;
    const h = harness(approved);
    h.controls.repo = { ...h.controls.repo, branch };
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "blocked");
    equal(
      result(h).reason,
      `Recorded worker worktree /repo/issue is ${text}; check out the issue's feature branch there, or run implement with an explicit worktree and branch, then drive`,
    );
    equal(h.controls.prepareCalls.length, 0);
    equal(h.calls.length, 0);
    equal((await loadState(h.context)).phase, "approved");
  }
});

function closedOutApproval(): LifecycleState {
  const state: LifecycleState = {
    ...implementing(),
    phase: "approved",
    workerWorktree: "/repo/issue",
  };
  const identity = { device: "1", inode: "2" };
  state.closeout = {
    id: "c6da10b0-c980-4b82-b920-000000000001",
    offerId: "c6da10b0-c980-4b82-b920-000000000002",
    issueDigest: issue.digest,
    attemptId: "c6da10b0-c980-4b82-b920-000000000003",
    origin: {
      session: "development",
      paneId: "terminal_1",
      server_instance: "server",
      provider: "claude",
      conversation: "conversation",
    },
    target: {
      worktree: "/repo/issue",
      branch: "issue-85",
      head,
      gitDir: "/repo/.git/worktrees/issue",
      branchIdentity: head,
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
    fence: {
      server_instance: "server",
      pane_id: "terminal_1",
      provider: "claude",
      conversation: "conversation",
      input_revision: 0,
      turn: "turn",
    },
    decision: "invalidated",
    request: "Close out",
    authorizedRequest: "Close out",
    git: "complete",
    pane: "closed",
  };
  return state;
}

Deno.test("driver keeps prepare_worktree when closeout may have removed the recorded worktree", async () => {
  const h = harness(closedOutApproval());
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "prepare_worktree");
  equal(
    result(h).reason,
    "Closeout may have removed the recorded worktree; deliberately select a new worktree with implement, then drive",
  );
  equal(h.controls.prepareCalls.length, 0);
  equal(h.calls.length, 0);
});

Deno.test("worktree adoption requires an explicit pair for legacy creation and after closeout", async () => {
  const legacy: LifecycleState = { ...implementing(), phase: "approved" };
  delete legacy.implementation;
  for (const state of [legacy, closedOutApproval()]) {
    const h = harness(state);
    for (
      const args of [{}, { worktree: "/repo/new" }, { branch: "issue-85-new" }]
    ) {
      await rejects(
        () =>
          adoptWorkerWorktree(h.context, state, args, {
            realPath: (path) => Promise.resolve(path),
            inspect: () => Promise.resolve(h.controls.repo),
            gitOperation: () => {
              throw new Error("Git must not run");
            },
          }),
        "supply both worktree and branch",
      );
    }
  }
  // An explicit closeout selection binds the new branch, not the old one.
  const state = closedOutApproval();
  const h = harness(state);
  const prepared: Record<string, unknown>[] = [];
  await adoptWorkerWorktree(h.context, state, {
    worktree: "/repo/issue-again",
    branch: "issue-85-again",
  }, {
    realPath: (path) => Promise.resolve(path),
    inspect: () => Promise.reject(new Error("Inspection must not run")),
    gitOperation: (_context, request) => {
      prepared.push(request);
      return Promise.resolve({
        ...h.controls.repo,
        worktree: request.worktree,
        branch: request.branch,
      });
    },
  });
  equal(prepared, [{
    action: "prepare",
    worktree: "/repo/issue-again",
    branch: "issue-85-again",
    issueKey: "default/85",
  }]);
  const adopted = await loadState(h.context);
  equal(adopted.implementation, {
    worktree: "/repo/issue-again",
    branch: "issue-85-again",
  });
  equal(adopted.workerWorktree, "/repo/issue-again");
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
    // Lost responses and writes after accepted evidence and a durable child
    // completion are re-observed within the same drive.
    const recovered = ["worker", "event", "completion"].includes(fault);
    equal(
      result(h).decision,
      recovered ? "authorize_plan_revision" : "blocked",
    );
    // A persistent lost response affects both the revision and its review.
    equal(
      h.controls.sleeps,
      fault === "worker" ? [1000, 1000] : recovered ? [1000] : [],
    );
    const reserved =
      DriverSchema.parse(h.resources.get("driver")).automaticRevision;
    equal(!!reserved, fault !== "reservation");
    if (fault === "dispatch") {
      // The failed first task write left the previous review saved; drive
      // must not classify that task as the failed dispatch.
      assert(
        result(h).reason.includes(
          `Automatic recovery not applicable: the saved assignment is not the expected task ${
            reserved!.assignment.id
          }`,
        ),
        result(h).reason,
      );
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
  assert(workerMaterial(replacement).includes(approval.request));
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
      // A later dispatched worker fails without accepted evidence and stays
      // blocked, so the lifecycle state below shows only the discard's effect.
      h.controls.submitEvidence = false;
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
      // The attestation stands in only because the worker cannot be observed.
      h.controls.stopError = UNOBSERVABLE_STOP;
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
      const persisted = {
        ...discard,
        basis: "completed_unusable",
        stopEvidence: "attested",
      };
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).discard,
        persisted,
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
            persisted,
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
    // Interrupted imports are re-observed once within drive; a cleanup
    // refusal without a structured transient guard stays blocked.
    equal(
      result(h).decision,
      fault === "cleanup" ? "blocked" : "authorize_merge",
    );
    equal(h.controls.sleeps, fault === "cleanup" ? [] : [1000]);
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
Deno.test("review repair workers inherit the issue worker choice", async () => {
  const h = await reviewRepairHarness(savedChoice);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "address_review_findings");
  assert(dispatchedRoles(h).includes("implement_review_fixes"));
  assert(!dispatchedRoles(h).includes("plan_review_fixes"));
  for (const call of h.calls) {
    equal(call.arguments?.kind, savedChoice.kind);
    equal(call.arguments?.model, savedChoice.model);
    equal(call.arguments?.effort, savedChoice.effort);
  }
  equal((await loadState(h.context)).workerAgent, savedChoice);
});
Deno.test("review repair loop repairs selected findings directly and re-reviews code before UX", async () => {
  const h = await reviewRepairHarness();
  const originalPlan = (await loadState(h.context)).plan;
  const count = h.calls.length;
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "address_review_findings");
  equal(h.calls.length, count + 2); // repair worker, code re-review
  equal(dispatchedRoles(h).slice(-2), [
    "implement_review_fixes",
    "review_code",
  ]);
  equal(h.controls.checkCalls.length, 0);
  let state = await loadState(h.context);
  equal(state.plan, originalPlan);
  equal(state.codeReview!.commit, "d".repeat(40));
  equal(state.uxReview, undefined);
  equal(state.reviewDecisions![0].result!.summary, "Repaired race");
  equal(state.reviewDecisions![0].plan, undefined);
  equal(state.reviewDecisions![0].approval, undefined);
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
  equal(dispatchedRoles(h).at(-1), "review_ux");
  equal(h.controls.checkCalls.length, 2); // the approved plan's checks
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
Deno.test("UX review runs once after code is clean; every re-review covers only its repair diff", async () => {
  const h = harness(driverImplementing());
  h.controls.deltaPaths = ["lib.rs"];
  const commits = ["d".repeat(40), "e".repeat(40)];
  const scopes = new Map<string, { role: string; scope: unknown }>();
  // First review of a role: nothing of that role reviewed before.
  const first = (
    task: ReturnType<typeof AgentTaskSchema.parse>,
    role: "review_code" | "review_ux",
  ) =>
    !priorImplementationFindings(task.before).some((p) =>
      p.ref.role === role
    ) &&
    !task.before.reviewHistory?.some((h) => h.role === role);
  const outcomes = (
    task: ReturnType<typeof AgentTaskSchema.parse>,
    role: "review_code" | "review_ux",
  ) =>
    priorFindingObligations(task.before, role).map((p) => ({
      finding: p.ref,
      outcome: "verified_fixed",
      rationale: "The repair diff resolves it",
      evidence: ["lib.rs:2"],
    }));
  h.controls.evidenceForRole = (task) => {
    const evidence = driverEvidence(task) as Record<string, unknown>;
    if (task.role === "review_code" || task.role === "review_ux") {
      scopes.set(task.id, { role: task.role, scope: task.continuity?.scope });
    }
    if (task.role === "review_code") {
      return {
        ...evidence,
        commit: h.controls.repo.head,
        priorFindings: outcomes(task, "review_code"),
        findings: first(task, "review_code")
          ? [{
            id: "race",
            severity: "high",
            description: "Lost work",
            affectedPaths: ["lib.rs"],
            evidence: ["lib.rs:1"],
          }]
          : [],
      };
    }
    if (task.role === "review_ux") {
      return {
        ...evidence,
        commit: h.controls.repo.head,
        priorFindings: outcomes(task, "review_ux"),
        findings: first(task, "review_ux")
          ? [{
            id: "copy",
            severity: "medium",
            description: "Error gives no next step",
            affectedPaths: ["lib.rs"],
            evidence: ["lib.rs:9"],
          }]
          : [],
      };
    }
    if (task.role === "implement_review_fixes") {
      // One commit per repair round, however often its evidence is read.
      h.controls.repo.head = commits[
        (task.before.reviewDecisions?.length ?? 1) - 1
      ];
      return { ...evidence, commit: h.controls.repo.head };
    }
    return evidence;
  };
  const decide = async (role: "review_code" | "review_ux", id: string) => {
    const state = await loadState(h.context);
    await saveState(
      h.context,
      "decide_review",
      transition(state, "decide_review", {
        id: result(h).id,
        reviewFingerprint: codeReviewFingerprint(state),
        request: `Repair ${id}`,
        repair: [{ role, findingId: id }],
        ignore: [],
      }),
    );
  };
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "address_review_findings");
  assert(!dispatchedRoles(h).includes("review_ux"));
  await decide("review_code", "race");
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "address_review_findings");
  equal(dispatchedRoles(h).slice(-3), [
    "implement_review_fixes",
    "review_code",
    "review_ux",
  ]);
  await decide("review_ux", "copy");
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_merge");
  equal(dispatchedRoles(h).slice(-3), [
    "implement_review_fixes",
    "review_code",
    "review_ux",
  ]);
  equal(
    dispatchedRoles(h).filter((role) => role === "review_ux").length,
    2,
  );
  const kinds = [...scopes.values()].map(({ role, scope }) => ({
    role,
    kind: (scope as { kind: string }).kind,
    base: (scope as { base?: string }).base,
    findings: ((scope as { findings?: { findingId: string }[] }).findings ??
      []).map((f) => f.findingId),
  }));
  equal(kinds, [
    { role: "review_code", kind: "full", base: undefined, findings: [] },
    {
      role: "review_code",
      kind: "delta",
      base: head,
      findings: ["race"],
    },
    { role: "review_ux", kind: "full", base: undefined, findings: [] },
    {
      role: "review_code",
      kind: "delta",
      base: "d".repeat(40),
      findings: [],
    },
    {
      role: "review_ux",
      kind: "delta",
      base: "d".repeat(40),
      findings: ["copy"],
    },
  ]);
  const state = await loadState(h.context);
  equal(state.codeReview!.scope?.kind, "delta");
  equal(state.uxReview!.scope?.kind, "delta");
  equal(state.verification!.commit, "e".repeat(40));
});
Deno.test("a standing auto-repair request repairs blocking findings without asking", async () => {
  const h = harness(driverImplementing());
  h.controls.evidenceForRole = (task) => {
    const evidence = driverEvidence(task) as Record<string, unknown>;
    if (task.role === "review_code") {
      return {
        ...evidence,
        commit: h.controls.repo.head,
        findings: !task.before.reviewDecisions?.length
          ? [{
            id: "race",
            severity: "high",
            description: "Lost work",
            affectedPaths: ["lib.rs"],
            evidence: ["lib.rs:1"],
          }]
          : [],
      };
    }
    if (task.role === "implement_review_fixes") {
      h.controls.repo.head = "d".repeat(40);
      return { ...evidence, commit: h.controls.repo.head };
    }
    return evidence;
  };
  await h.drive.execute({
    agent: options,
    autoRepair: { request: "Repair every blocking finding without asking" },
  }, h.context);
  equal(result(h).decision, "authorize_merge");
  const state = await loadState(h.context);
  const decision = state.reviewDecisions![0];
  equal(decision.source, "auto_repair");
  equal(decision.request, "Repair every blocking finding without asking");
  equal(decision.repair, [{ role: "review_code", findingId: "race" }]);
  assert(
    result(h).presentation.includes(
      "Review decision " + decision.id +
        ": Repair every blocking finding without asking",
    ),
  );
  equal(DriverSchema.parse(h.resources.get("driver")).autoRepair?.rounds, 1);
  await h.drive.execute({ autoRepair: { withdraw: true } }, h.context);
  equal(DriverSchema.parse(h.resources.get("driver")).autoRepair, undefined);
});
Deno.test("auto-repair hands back to the user after its round limit and outside its plan", async () => {
  const h = await reviewRepairHarness();
  const state = await loadState(h.context);
  // Undo the harness's manual decision: auto-repair makes it instead.
  await saveState(h.context, "rollback-fixture", {
    ...state,
    reviewDecisions: [],
  });
  const persistent = h.controls.evidenceForRole!;
  h.controls.evidenceForRole = (task) => {
    if (task.role === "implement_review_fixes") {
      h.controls.repo.head = `${task.before.reviewDecisions!.length}`
        .padStart(40, "d");
      return {
        ...driverEvidence(task) as object,
        summary: "Attempted repair",
        commit: h.controls.repo.head,
      };
    }
    return persistent(task);
  };
  await h.drive.execute({
    autoRepair: { request: "Keep repairing blocking findings" },
  }, h.context);
  equal(result(h).decision, "address_review_findings");
  assert(
    result(h).reason.includes("Auto-repair stopped after 3 rounds"),
    result(h).reason,
  );
  equal(
    dispatchedRoles(h).filter((r) => r === "implement_review_fixes").length,
    3,
  );
  equal(
    (await loadState(h.context)).reviewDecisions!.map((d) => d.source),
    ["auto_repair", "auto_repair", "auto_repair"],
  );
  // A request recorded for another plan version never applies.
  const driver = DriverSchema.parse(h.resources.get("driver"));
  h.resources.set("driver", {
    ...driver,
    autoRepair: { ...driver.autoRepair!, planVersion: 7, rounds: 0 },
  });
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "address_review_findings");
  assert(
    result(h).reason.includes("covered earlier requirements, plan or rollback"),
    result(h).reason,
  );
});
Deno.test("in-flight repair plans drain: a plan awaiting approval goes straight to repair", async () => {
  const h = await reviewRepairHarness();
  const selectedState = await loadState(h.context);
  // Saved by the earlier lifecycle: a repair plan waiting at approve_review_fixes.
  const planned = transition(selectedState, "plan_review_fixes", {
    ...driverPlan,
    findingMappings: selectedRepairFindings(selectedState).map((p) => ({
      finding: p.ref,
      steps: driverPlan.steps.map((step) => step.id),
      checks: driverPlan.checks.map((check) => check.id),
    })),
  });
  await saveState(h.context, "plan_review_fixes", planned);
  h.resources.set("handoff", {
    ...result(h),
    decision: "approve_review_fixes",
    state: planned,
  });
  equal(
    nextStep({ ...result(h), decision: "approve_review_fixes" }).startsWith(
      "Next: scripts/issue-lifecycle --item 85 drive (dispatches the repair worker",
    ),
    true,
  );
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "address_review_findings");
  assert(!dispatchedRoles(h).includes("plan_review_fixes"));
  assert(dispatchedRoles(h).includes("implement_review_fixes"));
  const fix = (await loadState(h.context)).reviewDecisions!.at(-1)!;
  assert(fix.plan);
  equal(fix.approval, undefined);
  assert(fix.result);
  // New repair planners are never dispatched.
  await rejects(
    () =>
      h.methods.run_agent.execute(
        { ...options, role: "plan_review_fixes" },
        h.context,
      ),
    "Repair planning is retired",
  );
});
Deno.test("review repair evidence recovers interrupted imports without dispatching a duplicate worker", async () => {
  const h = await reviewRepairHarness();
  h.controls.failEventOnce = true;
  const count = h.calls.length;
  await h.drive.execute({}, h.context);
  // The interrupted import is re-observed once within drive, then the code
  // re-review follows the implemented repair. No repair worker runs twice.
  equal(h.controls.sleeps, [1000]);
  equal(
    dispatchedRoles(h).filter((r) => r === "implement_review_fixes").length,
    1,
  );
  equal(h.calls.length, count + 2);
  equal(result(h).decision, "address_review_findings");
});
Deno.test("explicit restart preserves selected repair scope", async () => {
  const h = await reviewRepairHarness();
  h.controls.runError = "Provider unavailable";
  // Without accepted evidence, the failed worker stays blocked for restart.
  h.controls.submitEvidence = false;
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(old.role, "implement_review_fixes");
  h.controls.runError = undefined;
  h.controls.submitEvidence = true;
  await rejects(
    () =>
      h.methods.restart_agent.execute({
        ...options,
        role: "plan_review_fixes",
        taskId: old.id,
        request: "Restart as a planner",
      }, h.context),
    "Repair planning is retired",
  );
  await h.methods.restart_agent.execute({
    ...options,
    role: "implement_review_fixes",
    taskId: old.id,
    request: "Restart this repair worker",
  }, h.context);
  const state = await loadState(h.context);
  const fix = state.reviewDecisions!.at(-1)!;
  equal(fix.repair, [{ role: "review_code", findingId: "race" }]);
  equal(fix.approval, undefined);
  equal(fix.rollbackId, state.rollbackId);
  assert(fix.result);
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "address_review_findings");
});

Deno.test("explicit repair restart recovers imported scope after cleanup failure", async () => {
  const role = "implement_review_fixes" as const;
  const h = await reviewRepairHarness();
  h.controls.cleanupError = true;
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  const old = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(old.role, role);
  equal(old.status, "completed");
  equal(old.cleanup, "pending");
  const imported = await loadState(h.context);
  assert(imported.reviewDecisions!.at(-1)!.result);
  equal(imported.codeReview, undefined);
  equal(imported.uxReview, undefined);
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
  await rejects(
    () =>
      h.methods.restart_agent.execute({
        ...args,
        role: "plan_review_fixes" as const,
      }, h.context),
    "Repair planning is retired",
  );
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
  equal(fix.plan, undefined);
  equal(fix.approval, undefined);
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
  equal(result(h).decision, "address_review_findings");
});

function lastLine(h: ReturnType<typeof harness>) {
  return result(h).presentation.split("\n").at(-1)!;
}
Deno.test("every human handoff ends with its exact launcher command", async () => {
  const run = "Next: scripts/issue-lifecycle --item 85";
  // blocked
  let h = harness();
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "blocked");
  equal(
    lastLine(h),
    `Next: run scripts/issue-lifecycle --item 85 status and follow taskRecovery, then the matching recovery section`,
  );
  // approve_plan
  h = harness();
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  assert(
    lastLine(h).startsWith(
      `${run} approve --handoff ${
        result(h).id
      } --request '<the explicit user approval of plan version 1 and its checks>'`,
    ),
    lastLine(h),
  );
  // authorize_plan_revision: no transcription of handoff fields remains.
  h = harness();
  h.controls.evidenceForRole = blockedPlanEvidence;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "authorize_plan_revision");
  assert(
    lastLine(h).startsWith(`${run} drive --handoff ${result(h).id} --request`),
    lastLine(h),
  );
  assert(
    lastLine(h).includes(`adjudicate-plan --handoff ${result(h).id}`),
    lastLine(h),
  );
  assert(!result(h).presentation.includes("revisionApproval"));
  assert(!result(h).presentation.includes("reviewFingerprint"));
  assert(
    result(h).presentation.includes(
      "Revision authorization does not approve implementation or merge.",
    ),
  );
  // address_review_findings
  h = harness(implementing());
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
  assert(
    lastLine(h).startsWith(
      `${run} decide-review --handoff ${
        result(h).id
      } --request '<the user direction>'`,
    ),
    lastLine(h),
  );
  assert(lastLine(h).includes("Findings: review_code/bug."), lastLine(h));
  assert(
    lastLine(h).endsWith(
      "scripts/issue-lifecycle --item 85 drive --auto-repair --request '<the standing user instruction to repair every blocking finding>'",
    ),
    lastLine(h),
  );
  assert(
    result(h).presentation.includes("Which findings would you like addressed?"),
  );
  assert(!result(h).presentation.includes("Record your selection"));
  // authorize_merge, then integrate with its recorded authorization
  h = harness(driverImplementing());
  h.controls.evidenceForRole = driverEvidence;
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "authorize_merge");
  assert(
    lastLine(h).startsWith(
      `${run} ready --handoff ${
        result(h).id
      } --request '<the explicit user merge authorization for ${head}>'`,
    ),
    lastLine(h),
  );
  assert(!result(h).presentation.includes("Merge authorization for"));
  const state = await loadState(h.context);
  await saveState(
    h.context,
    "ready",
    transition(state, "ready", { commit: head, mergeRequest: "Merge it" }),
  );
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "integrate");
  assert(
    result(h).presentation.includes(
      `Merge authorization for ${head} (plan version 1): Merge it`,
    ),
    result(h).presentation,
  );
  equal(
    lastLine(h),
    `${run} merge, in the background or with a timeout of several hours (only ready and drive run merge automatically, unless --no-merge; other chained commands stop at this handoff; an existing attempt is never re-run automatically)`,
  );
});

Deno.test("handoff Next lines cover worktree, merge recovery and terminal decisions", () => {
  const run = "scripts/issue-lifecycle --item 85";
  const id = "a6da10b0-c980-4b82-b920-640238095f1a";
  const approved = { ...implementing(), phase: "approved" as const };
  delete approved.implementation;
  equal(
    nextStep({ id, decision: "prepare_worktree", state: approved }),
    `Next: ${run} implement --input-file <{"worktree": "...", "branch": "..."}> (legacy issue without a recorded worker worktree), then ${run} drive`,
  );
  assert(
    nextStep({
      id,
      decision: "prepare_worktree",
      state: { ...approved, workerWorktree: "/repo/issue" },
    }).includes(`"worktree": "/repo/issue"`),
  );
  const ready = transition(
    verify(
      transition(implementing(), "review_code", {
        planVersion: 1,
        commit: head,
        steps: [{ id: "fix", status: "implemented" }],
        findings: [],
      }),
    ),
    "ready",
    { commit: head, mergeRequest: "Merge" },
  );
  const attempt = {
    id: "b6da10b0-c980-4b82-b920-640238095f1a",
    originalHead: head,
    startedAt: timestamp,
  };
  equal(
    nextStep({ id, decision: "integrate", state: { ...ready, attempt } }),
    `Next: inspect merge attempt ${attempt.id} (an existing attempt is never re-run automatically), then ${run} merge`,
  );
  assert(
    nextStep({ id, decision: "summarize", state: ready }).includes(
      `${run} summarize --input-file`,
    ),
  );
  equal(
    nextStep({ id, decision: "resume_closeout", state: ready }),
    `Next: from a surviving pane, ${run} resume-closeout`,
  );
  equal(
    nextStep({ id, decision: "complete", state: ready }),
    "Next: None; present the completed result",
  );
  // A blocked handoff points at status's advisory taskRecovery block first.
  equal(
    nextStep({ id, decision: "blocked", state: ready }),
    `Next: run ${run} status and follow taskRecovery, then the matching recovery section`,
  );
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
  equal(task.contractVersion, 3);
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

Deno.test("new assignments reference private assignment files instead of inlining packets", async () => {
  const origin = "/repo/issue";
  const h = harness({ ...implementing(), workerWorktree: origin });
  h.controls.evidenceForRole = driverEvidence;
  await h.methods.run_agent.execute(
    { ...options, role: "review_code" },
    h.context,
  );
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(task.contractVersion, 3);
  const files = task.assignmentFiles!;
  equal(files.directory, `/repo/.git/yinz-lifecycle/assignments/${task.id}`);
  equal(files.files.packet.sha256, task.continuity!.packetDigest);
  assert(files.files.featureDiff && files.files.baseline);
  assert(task.prompt.includes("Assignment manifest ("));
  assert(task.prompt.includes("Swamp submission target:"));
  assert(
    task.prompt.includes(
      `--task-dir '${files.directory}' --assignment-digest ${
        task.submission!.assignmentDigest
      } <evidence.json|->`,
    ),
  );
  assert(
    task.prompt.includes(
      `python3 -I '/repo/control/scripts/issue_lifecycle/submit_evidence.py'`,
    ),
  );
  assert(task.prompt.includes("Assignment contract version 3."));
  assert(task.prompt.includes(`Start from ${files.files.template.path};`));
  assert(!task.prompt.includes("fixture full feature diff"));
  assert(
    !task.prompt.includes(
      "Authoritative repair/review packet (complete saved snapshot references): {",
    ),
  );
  assert(!task.prompt.includes("Evidence schema: "));
  // Published exactly once, as the task the prompt binds.
  equal(h.controls.publishCalls.map((t) => t.id), [task.id]);
  equal(h.controls.publishCalls[0].promptDigest, task.promptDigest);
});

Deno.test("assignment files are published before any prompt and failures send nothing", async () => {
  const h = harness();
  h.controls.failPublish = "Assignment directory must be private";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Assignment directory must be private",
  );
  equal(h.calls.filter((c) => c.modelType === "@yinz/agent").length, 0);
  const failed = AgentTaskSchema.parse(h.resources.get("agent-task"));
  assert(failed.lastError?.includes("Assignment directory must be private"));
  equal(failed.status, "running");
  equal(h.controls.publishCalls.length, 1);
  h.controls.failPublish = undefined;
  await h.methods.resume_agent.execute({}, h.context);
  const prompts = h.calls.filter((c) =>
    c.modelType === "@yinz/agent" && c.name === failed.behaviorName
  );
  equal(prompts.length, 1);
  equal(prompts[0].arguments?.prompt, failed.prompt);
  equal(h.controls.publishCalls.length, 2);
  equal((await loadState(h.context)).phase, "classified");
});

/** Rebuilds a never-delivered contract-3 task as a saved contract-2 task. */
async function savedContractTwo(h: ReturnType<typeof harness>) {
  h.controls.failPublish = "Stop before delivery";
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "Stop before delivery",
  );
  h.controls.failPublish = undefined;
  h.controls.publishCalls.length = 0;
  h.submissions.clear();
  const current = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const {
    assignmentFiles: _files,
    lastError: _error,
    ...rest
  } = current;
  const base = { ...rest, contractVersion: 2 as const };
  const submission = {
    ...current.submission!,
    assignmentDigest: evidenceDigest(
      assignmentCore(h.context, base, current.submission!.controlDir),
    ),
  };
  const prompt = taskPrompt(
    base.role,
    base.before,
    base.config.cwd,
    "/standards",
    submissionInstructions(base, submission),
    undefined,
    undefined,
    base.continuity,
  );
  const legacy = AgentTaskSchema.parse({
    ...base,
    submission,
    prompt,
    promptDigest: await promptDigest(prompt),
  });
  assert(!legacy.prompt.includes("Assignment manifest"));
  assert(legacy.prompt.includes("Assignment contract version 2."));
  assert(legacy.prompt.includes("Evidence schema: "));
  h.resources.set("agent-task", legacy);
  return legacy;
}

Deno.test("saved contract-2 tasks replay their original prompt byte-for-byte without assignment files", async () => {
  const h = harness();
  const legacy = await savedContractTwo(h);
  await h.methods.resume_agent.execute({}, h.context);
  const replay = h.calls.filter((c) =>
    c.modelType === "@yinz/agent" && c.name === legacy.behaviorName
  );
  equal(replay.map((c) => c.method), ["recover"]);
  equal(replay[0].arguments?.prompt, legacy.prompt);
  equal(h.controls.publishCalls.length, 0);
  const completed = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(completed.status, "completed");
  equal(completed.contractVersion, 2);
  equal(completed.assignmentFiles, undefined);
  equal((await loadState(h.context)).phase, "classified");
});

Deno.test("legacy continuation text is unchanged; contract-3 continuation uses the helper", async () => {
  const request = "Submit the existing findings";
  const finish =
    "Finish submission of the findings already established in this conversation. Preserve this task and its investigation; do not repeat the review or edit source or Git refs.";

  const legacyHarness = harness();
  const legacy = await savedContractTwo(legacyHarness);
  legacyHarness.controls.submitEvidence = false;
  await rejects(
    () => legacyHarness.methods.resume_agent.execute({}, legacyHarness.context),
    "submission_required",
  );
  legacyHarness.controls.submitEvidence = true;
  await legacyHarness.methods.continue_agent.execute(
    { taskId: legacy.id, request },
    legacyHarness.context,
  );
  const continued = AgentTaskSchema.parse(
    legacyHarness.resources.get("agent-task"),
  );
  equal(
    continued.continuation?.prompt,
    [finish, request, submissionInstructions(legacy, legacy.submission!)]
      .join("\n\n"),
  );
  assert(
    continued.continuation!.prompt.includes(
      "Submit evidence through Swamp before ending this task.",
    ),
  );
  equal(legacyHarness.controls.publishCalls.length, 0);
  equal(continued.status, "completed");

  const h = harness();
  h.controls.submitEvidence = false;
  await rejects(
    () =>
      h.methods.run_agent.execute({ ...options, role: "triage" }, h.context),
    "submission_required",
  );
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(h.controls.publishCalls.length, 1);
  h.controls.submitEvidence = true;
  let publishedBeforeContinue = false;
  h.controls.afterRun = () => {
    publishedBeforeContinue ||= h.controls.publishCalls.length === 2;
  };
  await h.methods.continue_agent.execute(
    { taskId: task.id, request },
    h.context,
  );
  const helper = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(
    helper.continuation?.prompt,
    [
      finish,
      request,
      submissionInstructions(task, task.submission!, task.assignmentFiles),
    ].join("\n\n"),
  );
  assert(helper.continuation!.prompt.includes("submit_evidence.py"));
  assert(helper.continuation!.prompt.includes("Assignment manifest ("));
  assert(
    !helper.continuation!.prompt.includes(
      "Write a temporary JSON file outside the source tree",
    ),
  );
  assert(publishedBeforeContinue, "assignment files published before continue");
  equal(helper.status, "completed");
});

/** A pane whose submission stalls on ordinary timeouts while other roles complete. */
function stallHarness(
  initial: LifecycleState,
  plan: {
    role: string;
    background: string[] | "flag" | "none";
    submitAt: "prompt" | number;
    completeAt?: number;
    // Observations (0 for the prompt) that report background work; default all.
    backgroundAt?: number[];
  },
) {
  const h = harness(initial);
  h.controls.evidenceForRole = driverEvidence;
  h.controls.submitEvidence = false;
  const cli: string[][] = [];
  const logs: { message: string; values: Record<string, unknown> }[] = [];
  h.context.logger = {
    info: (message, values) => logs.push({ message, values }),
  };
  let waits = 0;
  h.controls.agentRunner = async (args) => {
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
    const completed = {
      outcome: "completed",
      submission_id: "submission-1",
      acknowledged: true,
      agents: [agent],
      message: "Finished",
    };
    let value: Record<string, unknown>;
    if (args[3] === "spawn") {
      value = {
        outcome: "ok",
        spawned: { pane_id: "terminal_7", cwd: task.config.cwd },
      };
    } else if (args.includes("--until") && !args.includes("--submission")) {
      value = { outcome: "ok", agents: [agent] };
    } else if (task.role !== plan.role) {
      if (args[3] === "prompt") await h.submitEvidence();
      value = completed;
    } else {
      const wait = args[3] === "prompt" ? 0 : ++waits;
      if (plan.submitAt === (wait === 0 ? "prompt" : wait)) {
        await h.submitEvidence();
      }
      const background = plan.background === "none" ||
          plan.backgroundAt?.includes(wait) === false
        ? {}
        : plan.background === "flag"
        ? { background: true }
        : { background: true, background_tasks: plan.background };
      // Yinz reports background work on a timed-out wait only on the agent row.
      value = wait > 0 && wait === plan.completeAt ? completed : {
        outcome: "timed_out",
        submission_id: "submission-1",
        acknowledged: true,
        agents: [{
          ...agent,
          agent: { ...agent.agent, state: "Busy" },
          not_ready_reason: "unresolved_turn",
          ...background,
        }],
      };
    }
    return { code: 0, stdout: JSON.stringify(value), stderr: "" };
  };
  const timeoutLogs = () =>
    logs.filter((entry) => entry.message.includes("ordinary wait timeout"));
  return { ...h, cli, logs, timeoutLogs, waits: () => waits };
}

Deno.test("driver hands off accepted evidence once the pane reports only background work for bounded timeouts", async () => {
  for (
    const [background, shown] of [
      [["bsi01ktyo"], "background work [bsi01ktyo]"],
      [
        "flag",
        "background work without task IDs (Claude's session registry or an older Yinz build)",
      ],
    ] as const
  ) {
    const h = stallHarness(implementing(), {
      role: "implement",
      background: typeof background === "string" ? background : [
        ...background,
      ],
      submitAt: "prompt",
    });
    await h.drive.execute({ agent: options }, h.context);
    const handoff = result(h);
    equal(handoff.decision, "blocked");
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    equal(task.status, "running");
    equal(
      handoff.reason,
      `Pane task ${task.id} (implement) has an accepted Swamp evidence submission for commit ${head}, but pane terminal_7 still reports ${shown} after 2 consecutive ordinary wait timeouts. Yinz has not confirmed that the foreground turn ended; check that the pane is idle at its prompt before authorizing. drive will not wait on this task again or launch another worker. With the user's explicit authorization, import the accepted evidence without waiting on, prompting or closing the pane: accept-submission {taskId: "${task.id}", request: "<the user's actual authorization>"}. To observe the pane again instead, run resume-agent.`,
    );
    // The prompt timed out once, then one further observation timed out.
    equal(h.waits(), 1);
    const logs = h.timeoutLogs();
    equal(logs.length, 2);
    assert(logs[0].message.startsWith("Continuing original pane task"));
    assert(logs[1].message.startsWith("Stopped observing pane task"));
    equal(
      logs.map((entry) => entry.values),
      [1, 2].map((round) => ({
        task: task.id,
        role: "implement",
        rounds: round,
        stalledRounds: round,
        phase: "waiting",
        agentState: "Busy",
        backgroundTasks: typeof background === "string"
          ? ["background"]
          : background,
        submissionAccepted: true,
      })),
    );
    const stall = DriverSchema.parse(h.resources.get("driver")).backgroundStall;
    equal(stall?.taskId, task.id);
    equal(stall?.commit, head);
    // Repeated drive calls re-present the same handoff without observing,
    // prompting or launching anything.
    const cli = h.cli.length;
    for (let i = 0; i < 2; i++) {
      await h.drive.execute({}, h.context);
      equal(result(h), handoff);
    }
    equal(h.cli.length, cli);
    equal(h.calls.length, 2);
    equal(dispatchedRoles(h), ["implement"]);
  }
});

Deno.test("driver keeps observing timeouts when evidence is unaccepted or the pane reports no background work", async () => {
  for (
    const plan of [
      { background: ["job-1"], submitAt: 3, completeAt: 4 },
      { background: "none", submitAt: "prompt", completeAt: 4 },
    ] as const
  ) {
    const h = stallHarness(createState(issue), {
      role: "triage",
      background: typeof plan.background === "string" ? plan.background : [
        ...plan.background,
      ],
      submitAt: plan.submitAt,
      completeAt: plan.completeAt,
    });
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "approve_plan");
    equal(dispatchedRoles(h), ["triage", "plan", "review_plan"]);
    equal(h.waits(), 4);
    const logs = h.timeoutLogs();
    equal(logs.length, 4);
    assert(
      logs.every((entry) =>
        entry.message.startsWith("Continuing original pane task")
      ),
    );
    // A single accepted background-only round stays below the bound.
    equal(
      logs.map((entry) => entry.values.stalledRounds),
      plan.submitAt === "prompt" ? [0, 0, 0, 0] : [0, 0, 0, 1],
    );
    equal(
      logs.map((entry) => entry.values.submissionAccepted),
      plan.submitAt === "prompt"
        ? [true, true, true, true]
        : [false, false, false, true],
    );
    equal(
      DriverSchema.parse(h.resources.get("driver")).backgroundStall,
      undefined,
    );
  }
});

Deno.test("driver counts only consecutive background-only timeouts toward the stall bound", async () => {
  const h = stallHarness(createState(issue), {
    role: "triage",
    background: ["job-1"],
    submitAt: "prompt",
    backgroundAt: [0, 2],
    completeAt: 3,
  });
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "approve_plan");
  equal(dispatchedRoles(h), ["triage", "plan", "review_plan"]);
  equal(
    h.timeoutLogs().map((entry) => entry.values.stalledRounds),
    [1, 0, 1],
  );
  equal(
    DriverSchema.parse(h.resources.get("driver")).backgroundStall,
    undefined,
  );
});

Deno.test("a later resume-agent observation supersedes a recorded background stall", async () => {
  for (const importFails of [false, true]) {
    const plan: Parameters<typeof stallHarness>[1] = {
      role: "implement",
      background: ["bsi01ktyo"],
      submitAt: "prompt",
    };
    const h = stallHarness(implementing(), plan);
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "blocked");
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    // The pane finishes; resume-agent observes its completion.
    plan.completeAt = h.waits() + 1;
    if (importFails) {
      // Import refuses a dirty worktree, leaving the completed child unimported.
      h.controls.repo.clean = false;
      await rejects(
        () => h.methods.resume_agent.execute({}, h.context),
        "clean issue branch's current commit",
      );
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).status,
        "running",
      );
      h.controls.repo.clean = true;
    } else {
      await h.methods.resume_agent.execute({}, h.context);
      equal(
        AgentTaskSchema.parse(h.resources.get("agent-task")).status,
        "completed",
      );
    }
    const waits = h.waits();
    // drive imports or moves past the task instead of re-presenting the stall.
    await h.drive.execute({}, h.context);
    equal(result(h).decision, "authorize_merge");
    equal(dispatchedRoles(h), ["implement", "review_code", "review_ux"]);
    equal(h.waits(), waits);
    equal(
      DriverSchema.parse(h.resources.get("driver")).backgroundStall,
      undefined,
    );
    const archived = h.writes.filter((write) =>
      write.spec === "agent-task" && write.value.id === task.id
    ).at(-1)?.value;
    equal(archived?.status, "completed");
    equal(archived?.acceptance, undefined);
  }
});

Deno.test("accept-submission refuses a waiting pane without drive's recorded background stall", async () => {
  const h = stallHarness(implementing(), {
    role: "implement",
    background: ["bsi01ktyo"],
    submitAt: "prompt",
  });
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  // Without the stall a waiting pane may still be running its foreground turn.
  const { backgroundStall: stall, ...driver } = DriverSchema.parse(
    h.resources.get("driver"),
  );
  // A stall that a later child observation superseded is no longer drive's handoff.
  for (
    const recorded of [
      driver,
      {
        ...driver,
        backgroundStall: {
          ...stall!,
          child: { ...stall!.child, updatedAt: "2026-11-03T11:00:00.000Z" },
        },
      },
    ]
  ) {
    h.resources.set("driver", recorded);
    await rejects(
      () =>
        h.methods.accept_submission.execute({
          taskId: task.id,
          request: "Accept the submitted implementation",
        }, h.context),
      "background stall handoff",
    );
    equal(AgentTaskSchema.parse(h.resources.get("agent-task")), task);
  }
});

Deno.test("accept-submission imports stalled accepted implementation evidence through the ordinary import without touching the pane", async () => {
  const h = stallHarness(implementing(), {
    role: "implement",
    background: ["bsi01ktyo"],
    submitAt: "prompt",
  });
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  const before = await loadState(h.context);
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const cli = h.cli.length;
  const request = "Accept the submitted implementation; the pane is stuck";
  await h.methods.accept_submission.execute(
    h.methods.accept_submission.arguments.parse({
      taskId: task.id,
      request,
      workspace: "default",
      itemId: 85,
    }),
    h.context,
  );
  const accepted = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(accepted.status, "completed");
  equal(accepted.cleanup, undefined);
  equal(accepted.acceptance?.request, request);
  equal(accepted.acceptance?.childPhase, "waiting");
  equal(accepted.acceptance?.backgroundTasks, ["bsi01ktyo"]);
  assert(
    accepted.acceptance?.reason.includes("without waiting, input or closure"),
  );
  equal(accepted.evidence?.payload, {
    role: "implement",
    evidence: driverEvidence(task),
  });
  // Implementation reports leave lifecycle state unchanged, as on the happy path.
  equal(await loadState(h.context), before);
  assert(h.resources.has(`event-agent-${task.id}`));
  equal(h.cli.length, cli);
  equal(h.controls.cleanupCalls, 0);
  // An identical retry is a no-op; a different request is refused.
  await h.methods.accept_submission.execute(
    { taskId: task.id, request },
    h.context,
  );
  equal(h.cli.length, cli);
  await rejects(
    () =>
      h.methods.accept_submission.execute({
        taskId: task.id,
        request: "Another request",
      }, h.context),
    "different request",
  );
  // drive continues from the imported implementation exactly as after completion.
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "authorize_merge");
  equal(dispatchedRoles(h), ["implement", "review_code", "review_ux"]);
  assert(
    result(h).presentation.includes(
      "Delivered change: Repaired the affected caller and added a regression",
    ),
  );
});

Deno.test("accept-submission refuses blank requests, other tasks, unaccepted or mismatched evidence and stale worktrees", async () => {
  const stalled = async () => {
    const h = stallHarness(implementing(), {
      role: "implement",
      background: ["bsi01ktyo"],
      submitAt: "prompt",
    });
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "blocked");
    return { h, task: AgentTaskSchema.parse(h.resources.get("agent-task")) };
  };
  const request = "Accept the submitted implementation";
  const unchanged = (
    h: Awaited<ReturnType<typeof stalled>>["h"],
    task: ReturnType<typeof AgentTaskSchema.parse>,
  ) => equal(AgentTaskSchema.parse(h.resources.get("agent-task")), task);

  {
    const { h, task } = await stalled();
    for (const blank of ["", "   "]) {
      assert(
        !h.methods.accept_submission.arguments.safeParse({
          taskId: task.id,
          request: blank,
        }).success,
      );
      await rejects(
        () =>
          h.methods.accept_submission.execute(
            { taskId: task.id, request: blank },
            h.context,
          ),
        "request",
      );
    }
    await rejects(
      () =>
        h.methods.accept_submission.execute({
          taskId: "a6da10b0-c980-4b82-b920-999999999999",
          request,
        }, h.context),
      "current pane task's taskId",
    );
    unchanged(h, task);
  }

  for (
    const [tamper, message] of [
      [(saved: Record<string, unknown>) => {
        (saved.receipt as Record<string, unknown>).assignmentDigest = "f"
          .repeat(64);
      }, "Accepted receipt does not match"],
      [(saved: Record<string, unknown>) => {
        (saved.assignment as Record<string, unknown>).digest = "f".repeat(64);
      }, "superseded or different assignment"],
      [(saved: Record<string, unknown>) => {
        saved.status = "open";
        delete saved.receipt;
      }, "submission_required"],
    ] as const
  ) {
    const { h, task } = await stalled();
    const records = h.submissions.get(task.submission!.behaviorName)!;
    const saved = structuredClone(records.get("submission")) as Record<
      string,
      unknown
    >;
    tamper(saved);
    records.set("submission", saved);
    await rejects(
      () =>
        h.methods.accept_submission.execute(
          { taskId: task.id, request },
          h.context,
        ),
      message,
    );
    unchanged(h, task);
  }

  for (
    const patch of [{ clean: false }, { head: "c".repeat(40) }]
  ) {
    const { h, task } = await stalled();
    Object.assign(h.controls.repo, patch);
    await rejects(
      () =>
        h.methods.accept_submission.execute(
          { taskId: task.id, request },
          h.context,
        ),
      "clean issue branch's current commit",
    );
    unchanged(h, task);
  }

  {
    // Unaccepted evidence never stalls drive; the task stays observable.
    const h = stallHarness(createState(issue), {
      role: "triage",
      background: ["job-1"],
      submitAt: 99,
    });
    h.controls.agentRunner = ((runner) => (args, stdin, signal) => {
      // Stop the otherwise endless observation with a non-timeout failure.
      if (h.waits() >= 3 && args[3] === "wait") {
        return Promise.reject(new Error("Session disappeared"));
      }
      return runner(args, stdin, signal);
    })(h.controls.agentRunner!);
    await h.drive.execute({ agent: options }, h.context);
    equal(result(h).decision, "blocked");
    assert(!result(h).reason.includes("accept-submission"));
    const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
    await rejects(
      () =>
        h.methods.accept_submission.execute(
          { taskId: task.id, request },
          h.context,
        ),
      "submission_required",
    );
    equal(
      AgentTaskSchema.parse(h.resources.get("agent-task")).acceptance,
      undefined,
    );
  }
});

Deno.test("worker prompts require foreground commands and forbid background jobs", async () => {
  const h = harness();
  await h.methods.run_agent.execute({ ...options, role: "triage" }, h.context);
  const prompt = AgentTaskSchema.parse(h.resources.get("agent-task")).prompt;
  assert(
    prompt.includes(
      "Run every command and check in the foreground. Do not use run_in_background, Monitor, background subagents or any other background job. If a check exceeds the tool timeout, split it into smaller checks rather than backgrounding it.",
    ),
  );
});

Deno.test("an interrupted accept-submission import resumes through drive without observing the pane", async () => {
  const h = stallHarness(createState(issue), {
    role: "triage",
    background: ["bsi01ktyo"],
    submitAt: "prompt",
  });
  await h.drive.execute({ agent: options }, h.context);
  equal(result(h).decision, "blocked");
  const task = AgentTaskSchema.parse(h.resources.get("agent-task"));
  const waits = h.waits();
  h.controls.failTaskCompletionOnce = true;
  await rejects(
    () =>
      h.methods.accept_submission.execute({
        taskId: task.id,
        request: "Accept the submitted triage",
      }, h.context),
    "Fixture task completion write failed",
  );
  const applying = AgentTaskSchema.parse(h.resources.get("agent-task"));
  equal(applying.status, "applying");
  assert(applying.acceptance);
  equal((await loadState(h.context)).phase, "classified");
  await h.drive.execute({}, h.context);
  equal(result(h).decision, "approve_plan");
  equal(dispatchedRoles(h), ["triage", "plan", "review_plan"]);
  equal(h.waits(), waits);
  const archived = h.writes.filter((write) =>
    write.spec === "agent-task" && write.value.id === task.id
  ).at(-1)?.value;
  equal(archived?.status, "completed");
  equal(archived?.cleanup, undefined);
});
