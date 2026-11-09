import { launchReply, launchSupport, receipt } from "./launch_fixtures.ts";
import {
  AgentConfigSchema,
  type AgentContext,
  AgentStateSchema,
  cleanupAgent,
  type CommandResult,
  confirmFailureAgent,
  continueAgent,
  type Dependencies,
  EmptySchema,
  hooksReady,
  isTerminalFailure,
  recoverAgent,
  resumeAgent,
  runAgent,
  RunArgsSchema,
  statusAgent,
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
async function rejects(
  fn: () => Promise<unknown>,
  text: string,
): Promise<void> {
  try {
    await fn();
  } catch (error) {
    assert(
      error instanceof Error && error.message.includes(text),
      String(error),
    );
    return;
  }
  throw new Error(`Expected rejection containing ${text}`);
}
function agent(conversation = "conversation-1", kind = "claude") {
  return {
    pane_id: "terminal_7",
    session: "dev",
    agent: { kind, session_id: conversation || null, state: "idle" },
    cwd: "/work/issue",
    ready: true,
    status_available: true,
    prompt_supported: true,
  };
}
function ready(conversation = "conversation-1", kind = "claude") {
  return { outcome: "ok", agents: [agent(conversation, kind)] };
}
function completed(overrides: Record<string, unknown> = {}) {
  return {
    outcome: "completed",
    agents: [agent()],
    submission_id: "submission-1",
    message: "Implemented the fix.",
    ...overrides,
  };
}
const spawned = {
  outcome: "ok",
  spawned: { pane_id: "terminal_7", tab_id: 3, cwd: "/work/issue" },
};
const hooks = {
  code: 0,
  stdout:
    "Codex:\nscript: /hooks/script (current)\nuser: /hooks/config - 12 of 12 events wired\n",
  stderr: "",
};
type Reply = Record<string, unknown> | Record<string, unknown>[] | Error;

function harness(replies: Reply[], overrides: Record<string, unknown> = {}) {
  const records = new Map<string, Record<string, unknown>>();
  const writes: { name: string; value: Record<string, unknown> }[] = [];
  const calls: {
    args: string[];
    stdin?: string;
    signal?: AbortSignal;
    state?: Record<string, unknown>;
  }[] = [];
  const context: AgentContext = {
    globalArgs: AgentConfigSchema.parse({
      session: "dev",
      kind: "claude",
      cwd: "/work/issue",
      ...overrides,
    }),
    readResource: (name) => Promise.resolve(records.get(name) ?? null),
    writeResource: (_spec, name, value) => {
      records.set(name, structuredClone(value));
      writes.push({ name, value: structuredClone(value) });
      return Promise.resolve({ name });
    },
  };
  const dependencies: Dependencies = {
    now: () => "2026-11-03T12:00:00.000Z",
    id: () => "submission-1",
    realPath: (path) => Promise.resolve(path),
    run: (args, stdin, signal) => {
      if (args[3] === "launch-support") {
        return Promise.resolve({
          code: 0,
          stdout: JSON.stringify(launchSupport),
          stderr: "",
        });
      }
      calls.push({
        args,
        stdin,
        signal,
        state: structuredClone(records.get("state")),
      });
      const reply = replies.shift();
      if (reply instanceof Error) return Promise.reject(reply);
      if (!reply) throw new Error(`Unexpected command: ${args.join(" ")}`);
      if ("code" in reply) {
        const command = reply as CommandResult;
        try {
          command.stdout = JSON.stringify(
            launchReply(args, JSON.parse(command.stdout)),
          );
        } catch { /* Preserve malformed transport fixtures. */ }
        return Promise.resolve(command);
      }
      return Promise.resolve({
        code: 0,
        stdout: JSON.stringify(
          launchReply(args, reply as Record<string, unknown>),
        ),
        stderr: "",
      });
    },
  };
  return { context, dependencies, records, writes, calls, replies };
}

Deno.test("public method schemas accept Swamp's merged global arguments without passing them to the operation", () => {
  equal(
    RunArgsSchema.parse({
      prompt: "Fix it",
      session: "dev",
      kind: "claude",
      cwd: "/work/issue",
    }),
    { prompt: "Fix it" },
  );
  equal(
    EmptySchema.parse({ session: "dev", kind: "claude", cwd: "/work/issue" }),
    {},
  );
});

Deno.test("agent run saves launch and submission intents before sending and pins the selected conversation", async () => {
  const h = harness([spawned, ready(), completed()]);
  await runAgent({ prompt: "  Fix the issue.\n" }, h.context, h.dependencies);
  equal(h.calls[0].state?.phase, "launching");
  assert(h.calls[0].args.includes("--cwd"));
  assert(!h.calls[0].args.includes("--branch"));
  assert(!h.calls[0].args.includes("--permission-mode"));
  const prompt = h.calls[2];
  equal(prompt.state?.phase, "submitting");
  equal(prompt.state?.submissionId, "submission-1");
  equal(
    prompt.args[prompt.args.indexOf("--conversation") + 1],
    "conversation-1",
  );
  equal(prompt.stdin, "  Fix the issue.\n");
  equal(h.records.get("result")?.message, "Implemented the fix.");
  await rejects(
    () => runAgent({ prompt: "  Fix the issue.\n" }, h.context, h.dependencies),
    "use resume",
  );
  await rejects(
    () => runAgent({ prompt: "Different task" }, h.context, h.dependencies),
    "another prompt",
  );
  equal(h.calls.length, 3);
});

Deno.test("agent resume retrieves a lost submission instead of sending it again", async () => {
  const h = harness([
    spawned,
    ready(),
    new Error("connection lost"),
    completed(),
  ]);
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "connection lost",
  );
  equal(h.records.get("state")?.submissionId, "submission-1");
  await resumeAgent(h.context, h.dependencies);
  const last = h.calls[3].args;
  assert(last.includes("wait") && last.includes("--submission"));
  equal(last[last.indexOf("--submission") + 1], "submission-1");
  equal(last[last.indexOf("--conversation") + 1], "conversation-1");
  equal(h.calls.filter((call) => call.args.includes("prompt")).length, 1);
});

Deno.test("a failed result write keeps the original submission resumable", async () => {
  const h = harness([spawned, ready(), completed(), completed()]);
  const writeResource = h.context.writeResource;
  let failResult = true;
  h.context.writeResource = (spec, name, value) => {
    if (name === "result" && failResult) {
      failResult = false;
      return Promise.reject(new Error("Injected result write failure"));
    }
    return writeResource(spec, name, value);
  };
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "Injected result write failure",
  );
  assert(!h.records.has("result"));
  assert(h.records.get("state")?.phase !== "completed");
  equal(h.records.get("state")?.submissionId, "submission-1");
  const resumed = await resumeAgent(h.context, h.dependencies);
  equal(resumed.dataHandles, [{ name: "result" }, { name: "state" }]);
  equal(h.records.get("state")?.phase, "completed");
  equal(h.records.get("state")?.lastError, undefined);
  equal(h.records.get("result")?.message, "Implemented the fix.");
  const last = h.calls[3].args;
  assert(last.includes("wait"));
  equal(last[last.indexOf("--submission") + 1], "submission-1");
  equal(last[last.indexOf("--conversation") + 1], "conversation-1");
  equal(h.calls.filter((call) => call.args.includes("prompt")).length, 1);
  await resumeAgent(h.context, h.dependencies);
  equal(h.calls.length, 4);
});

Deno.test("uncertain spawn never respawns automatically", async () => {
  const h = harness([new Error("lost spawn response"), {
    outcome: "status_unavailable",
    launch: receipt("server-test.submission-1", {
      outcome: "unknown",
      process: "unknown",
      disposition: "unknown",
    }),
  }]);
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "lost spawn response",
  );
  await rejects(
    () => resumeAgent(h.context, h.dependencies),
    "Launch unknown",
  );
  equal(h.calls.filter((call) => call.args[3] === "spawn").length, 1);
  equal(h.records.get("state")?.phase, "launching");
});

Deno.test("unsupported effort rejection is classified with its response and never relaunched", async () => {
  const rejected = {
    outcome: "rejected",
    message: "Unsupported effort: ultra",
    spawned: null,
    agents: [],
    submission_id: null,
    background_tasks: [],
  };
  for (const code of [0, 1]) {
    const h = harness([{
      code,
      stdout: JSON.stringify(rejected),
      stderr: "",
    }], { effort: "ultra" });
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "discard-agent",
    );
    const state = AgentStateSchema.parse(h.records.get("state"));
    equal(state.phase, "launch_rejected");
    equal(state.outcome, "rejected");
    equal(state.message, rejected.message);
    equal(state.config.effort, "ultra");
    equal(JSON.parse(state.rawResponse!).message, rejected.message);
    equal(state.paneId, undefined);
    equal(state.submissionId, undefined);
    assert(h.writes.some(({ value }) => value.phase === "launch_rejected"));
    assert(!h.records.has("result"));
    await rejects(
      () => resumeAgent(h.context, h.dependencies),
      "discard-agent",
    );
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "already exists",
    );
    equal(h.calls.length, 1);
  }
});

Deno.test("spawn rejection with resources and malformed or uncertain responses stay unresolved", async () => {
  const replies: Reply[] = [
    ...[
      { spawned: spawned.spawned },
      { agents: [agent()] },
      { submission_id: "submission-1" },
      { submission_id: "" },
      { background_tasks: ["worker-1"] },
      { acknowledged: true },
      { turn_id: "turn-1" },
      { project: { name: "created", root: "/work/created" } },
      { restarted: { pane_id: "terminal_7" } },
      { launch_resources: { tab_id: 3 } },
      { agents: null },
      { spawned: { pane_id: "invalid" } },
      { background_tasks: "malformed" },
    ].map((fields) => ({ outcome: "rejected", ...fields })),
    { outcome: "failed" },
    { outcome: "timed_out" },
    { outcome: "interrupted" },
    { code: 1, stdout: "not JSON", stderr: "connection failed" },
    new Error("lost spawn response"),
  ];
  for (const reply of replies) {
    const h = harness([reply, {
      outcome: "status_unavailable",
      launch: receipt("server-test.submission-1", {
        outcome: "unknown",
        process: "unknown",
        disposition: "unknown",
      }),
    }]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "",
    );
    equal(h.records.get("state")?.phase, "launching");
    assert(!h.records.has("result"));
    await rejects(
      () => resumeAgent(h.context, h.dependencies),
      "Launch unknown",
    );
    equal(h.calls.filter((call) => call.args[3] === "spawn").length, 1);
  }
});

Deno.test("failed, timed out, input-needed and expired submissions preserve state without publishing success", async () => {
  for (
    const outcome of [
      "failed",
      "timed_out",
      "needs_input",
      "interrupted",
      "target_changed",
      "target_closed",
      "rejected",
    ]
  ) {
    const failure = completed({ outcome, message: "Inspect the pane" });
    const h = harness([spawned, ready(), {
      code: 1,
      stdout: JSON.stringify(failure),
      stderr: "",
    }, failure]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      outcome,
    );
    assert(!h.records.has("result"));
    equal(h.records.get("state")?.outcome, outcome);
    await rejects(() => resumeAgent(h.context, h.dependencies), outcome);
    equal(h.calls.filter((call) => call.args.includes("prompt")).length, 1);
  }
});

Deno.test("provider completion requires matching submission and unchanged conversation", async () => {
  for (
    const [value, error] of [
      [completed({ submission_id: "someone-else" }), "saved submission"],
      [completed({ agents: [agent("replacement")] }), "conversation changed"],
    ] as const
  ) {
    const h = harness([spawned, ready(), value]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      error,
    );
    assert(!h.records.has("result"));
  }
});

Deno.test("missing final text completes the provider turn without reusing stale text or retrieving again", async () => {
  for (const message of [null, undefined, "", " \n\t"]) {
    for (const recover of [false, true]) {
      const h = harness([
        spawned,
        { ...ready(), message: "Old status text" },
        ...(recover ? [new Error("Lost response")] : []),
        completed({ message }),
      ]);
      if (recover) {
        await rejects(
          () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
          "Lost response",
        );
      }
      if (recover) await resumeAgent(h.context, h.dependencies);
      else await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
      const state = AgentStateSchema.parse(h.records.get("state"));
      equal(state.phase, "completed");
      equal(state.outcome, "completed");
      equal(state.message, undefined);
      equal(state.backgroundTasks, []);
      equal(h.records.get("result")?.message, undefined);
      const count = h.calls.length;
      await resumeAgent(h.context, h.dependencies);
      equal(h.calls.length, count);
    }
  }
});

Deno.test("missing final text does not mark mismatched or active submissions terminal", async () => {
  for (
    const patch of [
      { submission_id: "other" },
      { agents: [agent("other")] },
      { agents: [{ ...agent(), pane_id: "terminal_8" }] },
      { agents: [agent("conversation-1", "muse")] },
      { background_tasks: ["worker"] },
      { agents: [{ ...agent(), background_tasks: ["worker"] }] },
    ]
  ) {
    const h = harness([
      spawned,
      ready(),
      completed({ message: null, ...patch }),
      new Error("Background still active"),
    ]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "",
    );
    assert(h.records.get("state")?.phase !== "completed_unusable");
    assert(!h.records.has("result"));
  }
});

Deno.test("background interim completion waits for the original conversation's final idle answer", async () => {
  for (
    const patch of [
      { background_tasks: ["worker-1"] },
      { background: true },
      { agents: [{ ...agent(), background: true }] },
    ]
  ) {
    const h = harness([
      spawned,
      ready(),
      completed({ ...patch, message: "Working" }),
      { ...ready(), message: "Final answer" },
    ]);
    await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
    equal(h.calls[3].state?.phase, "background");
    assert(h.calls[3].args.includes("--until"));
    assert(!h.calls[3].args.includes("--submission"));
    equal(h.records.get("result")?.message, "Final answer");
  }
});

Deno.test("empty background completion completes the provider turn without reusing interim text", async () => {
  for (const message of [null, undefined, "", " \n\t"]) {
    for (const recover of [false, true]) {
      const h = harness([
        spawned,
        ready(),
        completed({ background_tasks: ["worker"], message: "Working" }),
        ...(recover ? [new Error("Lost observation")] : []),
        { ...ready(), message },
      ]);
      if (recover) {
        await rejects(
          () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
          "Lost observation",
        );
        equal(h.records.get("state")?.phase, "background");
      }
      if (recover) await resumeAgent(h.context, h.dependencies);
      else await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
      const state = AgentStateSchema.parse(h.records.get("state"));
      equal(state.phase, "completed");
      equal(state.outcome, "completed");
      equal(state.message, undefined);
      equal(state.backgroundTasks, []);
      equal(h.records.get("result")?.message, undefined);
      const count = h.calls.length;
      await resumeAgent(h.context, h.dependencies);
      equal(h.calls.length, count);
    }
  }
});

Deno.test("empty background observations require matching identity, idle state and no workers", async () => {
  for (
    const patch of [
      { agents: [agent("replacement")] },
      { agents: [{ ...agent(), pane_id: "terminal_8" }] },
      { agents: [{ ...agent(), session: "other" }] },
      { agents: [agent("conversation-1", "muse")] },
      {
        agents: [{ ...agent(), agent: { ...agent().agent, state: "working" } }],
      },
      { agents: [{ ...agent(), background_tasks: ["worker"] }] },
      { background_tasks: ["worker"] },
    ]
  ) {
    const h = harness([
      spawned,
      ready(),
      completed({ background_tasks: ["worker"] }),
      { ...ready(), message: null, ...patch },
    ]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "",
    );
    equal(h.records.get("state")?.phase, "background");
    assert(!h.records.has("result"));
  }
});

Deno.test("background timeout does not turn an interim answer into success", async () => {
  const h = harness([
    spawned,
    ready(),
    completed({ background_tasks: ["worker-1"] }),
    { outcome: "timed_out", agents: [agent()], background_tasks: ["worker-1"] },
  ]);
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "timed_out",
  );
  assert(!h.records.has("result"));
  equal(h.records.get("state")?.phase, "background");
});

Deno.test("readiness and spawn directory are checked before any prompt", async () => {
  const h = harness([spawned, {
    outcome: "ok",
    agents: [{ ...agent(), ready: false, status_available: false }],
  }]);
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "readiness",
  );
  equal(h.calls.length, 2);
  const wrong = harness([{
    ...spawned,
    spawned: { ...spawned.spawned, cwd: "/work/another" },
  }]);
  await rejects(
    () => runAgent({ prompt: "Fix it" }, wrong.context, wrong.dependencies),
    "directory differs",
  );
  equal(wrong.calls.length, 1);
});

Deno.test("an idle draft does not block an explicitly requested guarded prompt", async () => {
  const h = harness([spawned, {
    outcome: "ok",
    agents: [{
      ...agent(),
      ready: false,
      not_ready_reason: "unconsumed_input",
    }],
  }, completed()]);
  await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  equal(h.records.get("result")?.outcome, "completed");
});

Deno.test("empty optional workflow settings do not override provider defaults or permissions", async () => {
  const h = harness([spawned, ready(), completed()], {
    model: "",
    effort: " ",
    permissionMode: "",
  });
  await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  const launch = h.calls[0].args;
  assert(!launch.includes("--model"));
  assert(!launch.includes("--effort"));
  assert(!launch.includes("--permission-mode"));
});

Deno.test("Muse first submission can bind its conversation, while other providers require one", async () => {
  const h = harness([
    hooks,
    spawned,
    ready("", "muse"),
    completed({ agents: [agent("muse-1", "muse")] }),
  ], { kind: "muse" });
  await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  assert(!h.calls[3].args.includes("--conversation"));
  equal(h.records.get("result")?.conversationId, "muse-1");
  const missing = harness([spawned, ready("")]);
  await rejects(
    () => runAgent({ prompt: "Fix it" }, missing.context, missing.dependencies),
    "conversation to guard",
  );
  equal(missing.calls.length, 2);
});

Deno.test("AGY preflight checks project hooks while user-only providers omit the project", async () => {
  for (const kind of ["agy", "codex", "muse"]) {
    const h = harness([
      {
        ...hooks,
        stdout:
          `${kind}:\nscript: /hooks/script (current)\nproject: /work/issue/.agents/hooks.json - 12 of 12 events wired\n`,
      },
      spawned,
      ready("conversation-1", kind),
      completed({ agents: [agent("conversation-1", kind)] }),
    ], { kind });
    await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
    equal(h.calls[0].args, [
      "--session",
      "dev",
      "hooks",
      "status",
      "--agent",
      kind,
      ...(kind === "agy" ? ["--project-dir", "/work/issue"] : []),
    ]);
    assert(h.records.has("result"));
  }
  for (const status of ["missing", "older than this yinz"]) {
    const h = harness([{
      ...hooks,
      stdout: hooks.stdout.replace("(current)", `(${status})`),
    }], { kind: "agy" });
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "hooks",
    );
    equal(h.calls.length, 1);
  }
});

Deno.test("missing or stale hooks fail before launch and are never installed automatically", async () => {
  assert(hooksReady(hooks.stdout));
  assert(
    !hooksReady(hooks.stdout.replace("(current)", "(older than this yinz)")),
  );
  assert(!hooksReady(hooks.stdout.replace("12 of 12", "11 of 12")));
  const h = harness([{
    ...hooks,
    stdout: hooks.stdout.replace("(current)", "(missing)"),
  }], { kind: "codex" });
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "hooks",
  );
  equal(h.calls.length, 1);
  assert(!h.calls[0].args.includes("install"));
});

Deno.test("status observation cannot replace the completed task's answer", async () => {
  const h = harness([spawned, ready(), completed(), {
    ...ready(),
    message: "Pane is idle",
  }]);
  await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  await statusAgent(h.context, h.dependencies);
  await resumeAgent(h.context, h.dependencies);
  equal(h.records.get("result")?.message, "Implemented the fix.");
  equal(AgentStateSchema.parse(h.records.get("state")).phase, "completed");
  assert(h.records.has("observation"));
});

Deno.test("a failed concurrent status cannot overwrite task state advanced by another invocation", async () => {
  const h = harness([spawned, ready(), completed()]);
  await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  const original = AgentStateSchema.parse(h.records.get("state"));
  const advanced = {
    ...original,
    phase: "waiting",
    message: "Concurrent progress",
  };
  const dependencies = {
    ...h.dependencies,
    run: () => {
      h.records.set("state", advanced);
      return Promise.resolve({
        code: 1,
        stdout: JSON.stringify({ outcome: "target_changed" }),
        stderr: "",
      });
    },
  };
  await rejects(() => statusAgent(h.context, dependencies), "target_changed");
  equal(h.records.get("state"), advanced);
  equal(h.records.get("observation")?.outcome, "target_changed");
});

Deno.test("completed resume checks every stored identity field", async () => {
  for (
    const field of [
      "session",
      "kind",
      "cwd",
      "paneId",
      "conversationId",
      "submissionId",
      "promptDigest",
    ]
  ) {
    const h = harness([spawned, ready(), completed()]);
    await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
    h.records.set("result", {
      ...h.records.get("result"),
      [field]: field === "paneId" ? "terminal_8" : "different",
    });
    await rejects(
      () => resumeAgent(h.context, h.dependencies),
      "Stored completion",
    );
    equal(h.calls.length, 3);
  }
});

Deno.test("cancellation reaches every CLI call and keeps the submission available for observation", async () => {
  const h = harness([
    spawned,
    ready(),
    new Error("observation cancelled"),
    completed(),
  ]);
  const controller = new AbortController();
  h.context.signal = controller.signal;
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "observation cancelled",
  );
  assert(h.calls.every((call) => call.signal === controller.signal));
  equal(h.records.get("state")?.submissionId, "submission-1");
  await resumeAgent(h.context, h.dependencies);
  equal(h.calls.filter((call) => call.args.includes("prompt")).length, 1);
});

Deno.test("recovery initializes an unstarted task once and validates existing intent", async () => {
  const h = harness([spawned, ready(), completed()]);
  await recoverAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  // Swamp serializes actions on this child; a second caller sees the first write.
  await recoverAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  equal(h.calls.length, 3);
  await rejects(
    () => recoverAgent({ prompt: "Other" }, h.context, h.dependencies),
    "prompt does not match",
  );
  h.context.globalArgs.session = "other";
  await rejects(
    () => recoverAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "configuration changed",
  );
});

Deno.test("terminal failure requires correlated failure and a fresh stopped observation", async () => {
  const failure = completed({ outcome: "failed", acknowledged: true });
  const h = harness([spawned, ready(), failure, failure, ready()]);
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "has not completed",
  );
  assert(isTerminalFailure(AgentStateSchema.parse(h.records.get("state"))));
  await confirmFailureAgent(h.context, h.dependencies);
  equal(h.records.get("state")?.outcome, "failed");
  equal(h.records.get("observation")?.outcome, "ok");
  assert(!h.records.has("result"));
  equal(
    h.calls[3].args[h.calls[3].args.indexOf("--submission") + 1],
    "submission-1",
  );
});

Deno.test("failure discard rejects uncertain evidence, replacements and background work", async () => {
  const failure = completed({ outcome: "failed", acknowledged: true });
  const cases: [Reply, Reply | undefined, string][] = [
    [completed({ outcome: "timed_out" }), undefined, "No confirmed terminal"],
    [
      completed({ outcome: "failed", submission_id: "other" }),
      undefined,
      "No confirmed terminal",
    ],
    [
      completed({ outcome: "failed", agents: [agent("replacement")] }),
      undefined,
      "No confirmed terminal",
    ],
    [new Error("connection lost"), undefined, "connection lost"],
    [failure, { ...ready(), background_tasks: ["job"] }, "uncertain or active"],
    [failure, {
      outcome: "ok",
      agents: [{ ...agent(), background_tasks: ["job"] }],
    }, "uncertain or active"],
    [failure, {
      outcome: "ok",
      agents: [{ ...agent(), agent: { ...agent().agent, state: "busy" } }],
    }, "uncertain or active"],
    [failure, ready("replacement"), "conversation changed"],
    [failure, { code: 1, stdout: "broken", stderr: "" }, "no valid response"],
  ];
  for (const [recheck, observation, error] of cases) {
    const h = harness([
      spawned,
      ready(),
      failure,
      recheck,
      ...(observation ? [observation] : []),
    ]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "has not completed",
    );
    await rejects(() => confirmFailureAgent(h.context, h.dependencies), error);
    assert(!h.records.has("result"));
  }
});

function unsent(overrides: Record<string, unknown> = {}) {
  return completed({
    outcome: "target_changed",
    prompt_delivery: "not_sent",
    acknowledged: false,
    turn_id: null,
    message: "Preparation failed; inspect the composer before reuse.",
    ...overrides,
  });
}

Deno.test("unsent failure confirmation retrieves the original and preserves it separately from fresh status", async () => {
  for (
    const outcome of [
      "target_changed",
      "target_closed",
      "status_unavailable",
      "interrupted",
      "timed_out",
      "failed",
    ]
  ) {
    const failure = unsent({ outcome });
    const h = harness([spawned, ready(), failure, failure, {
      outcome: "ok",
      agents: [{
        ...agent(),
        ready: false,
        not_ready_reason: "unconsumed_input",
      }],
    }]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "has not completed",
    );
    assert(isTerminalFailure(AgentStateSchema.parse(h.records.get("state"))));
    await confirmFailureAgent(h.context, h.dependencies);
    equal(JSON.parse(String(h.records.get("state")?.rawResponse)), failure);
    equal(h.records.get("observation")?.outcome, "ok");
    equal(h.calls.map((call) => call.args[3]), [
      "spawn",
      "wait",
      "prompt",
      "wait",
      "status",
    ]);
    assert(h.calls[3].args.includes("submission-1"));
    assert(h.calls[3].args.includes("conversation-1"));
    assert(!h.records.has("result"));
  }
});

Deno.test("unsent confirmation rejects legacy, malformed, uncorrelated and uncertain delivery evidence", async () => {
  const cases = [
    { prompt_delivery: undefined },
    { prompt_delivery: undefined, outcome: "failed" },
    { prompt_delivery: "started" },
    { prompt_delivery: "started", outcome: "failed" },
    { prompt_delivery: null },
    { prompt_delivery: "never_sent" },
    { outcome: "ok" },
    { outcome: "submitted" },
    { outcome: "completed" },
    { outcome: "needs_input" },
    { outcome: "rejected" },
    { submission_id: undefined },
    { submission_id: "unknown" },
    { agents: [] },
    { agents: [agent(), agent()] },
    { agents: [{ ...agent(), pane_id: "terminal_8" }] },
    { agents: [{ ...agent(), session: "other" }] },
    { agents: [agent("other")] },
    { agents: [agent("conversation-1", "codex")] },
    { acknowledged: true },
    { turn_id: "turn-1" },
    { turn_id: "" },
    { background_tasks: ["work"] },
    { agents: [{ ...agent(), background_tasks: ["work"] }] },
  ];
  for (const patch of cases) {
    const failure = unsent(patch);
    const h = harness([spawned, ready(), unsent(), failure]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "has not completed",
    );
    await rejects(
      () => confirmFailureAgent(h.context, h.dependencies),
      patch.prompt_delivery === null || patch.prompt_delivery === "never_sent"
        ? "no valid agent response"
        : "No confirmed terminal",
    );
    assert(!isTerminalFailure(AgentStateSchema.parse(h.records.get("state"))));
    assert(!h.records.has("observation"));
    assert(!h.records.has("result"));
  }
});

Deno.test("unsent discard requires fresh idle identity and drained writers but permits a leftover marker", async () => {
  const statuses = [
    { ...agent(), ready: false, not_ready_reason: "clearing_draft" },
    { ...agent(), ready: true, not_ready_reason: "pending_submission" },
    { ...agent(), ready: false, not_ready_reason: "unresolved_turn" },
    { ...agent(), ready: false },
    { ...agent(), status_available: false },
    { ...agent(), prompt_supported: false },
    { ...agent(), agent: { ...agent().agent, state: "busy" } },
    { ...agent(), agent: { ...agent().agent, state: "waiting" } },
    { ...agent(), background_tasks: ["work"] },
    { ...agent(), session: "other" },
    { ...agent(), pane_id: "terminal_8" },
    agent("other"),
    agent("conversation-1", "codex"),
  ];
  for (const status of statuses) {
    const h = harness([spawned, ready(), unsent(), unsent(), {
      outcome: "ok",
      agents: [status],
    }]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "has not completed",
    );
    await rejects(() => confirmFailureAgent(h.context, h.dependencies), "");
    equal(JSON.parse(String(h.records.get("state")?.rawResponse)), unsent());
    assert(!h.records.has("result"));
  }
});

Deno.test("control placement resolves tab zero and current tab before each launch, with no lookup on resume", async () => {
  for (const tab of [0, 9]) {
    const h = harness([
      [{ id: 1, is_plugin: true, tab_id: 77 }, {
        id: 1,
        is_plugin: false,
        tab_id: tab,
      }],
      spawned,
      ready(),
      completed(),
    ], { control: { session: "dev", paneId: "terminal_1" } });
    await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
    equal(h.calls[0].args, [
      "--session",
      "dev",
      "action",
      "list-panes",
      "--json",
    ]);
    const spawn = h.calls[1].args;
    equal(spawn[spawn.indexOf("--tab") + 1], String(tab));
    const count = h.calls.length;
    await resumeAgent(h.context, h.dependencies);
    equal(h.calls.length, count);
  }
});

Deno.test("unresolved control panes reject launches without falling back to automatic placement", async () => {
  for (
    const reply of [
      [],
      [{ id: 1, is_plugin: true, tab_id: 0 }],
      [{ id: 2, is_plugin: false, tab_id: 0 }],
      { code: 1, stdout: "", stderr: "session missing" },
      { code: 0, stdout: "invalid JSON", stderr: "" },
      [{ id: 1, is_plugin: false, tab_id: null }],
    ]
  ) {
    const h = harness([reply], {
      control: { session: "dev", paneId: "terminal_1" },
    });
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "",
    );
    assert(h.calls.every((call) => !call.args.includes("spawn")));
    equal(h.records.get("state")?.phase, "preflight");
  }
  const mismatch = harness([], {
    control: { session: "other", paneId: "terminal_1" },
  });
  await rejects(
    () =>
      runAgent({ prompt: "Fix it" }, mismatch.context, mismatch.dependencies),
    "different session",
  );
  equal(mismatch.calls.length, 0);
});

Deno.test("a controlled worker resumes its original submission without resolving or spawning again", async () => {
  const h = harness([
    [{ id: 1, is_plugin: false, tab_id: 0 }],
    spawned,
    ready(),
    new Error("connection lost"),
    completed(),
  ], { control: { session: "dev", paneId: "terminal_1" } });
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "connection lost",
  );
  await resumeAgent(h.context, h.dependencies);
  equal(h.calls.map((call) => call.args[3]), [
    "list-panes",
    "spawn",
    "wait",
    "prompt",
    "wait",
  ]);
  const wait = h.calls[4].args;
  equal(wait[wait.indexOf("--submission") + 1], "submission-1");
});

Deno.test("controller stack launch uses the owner rather than a tab snapshot", async () => {
  const h = harness([
    [{ id: 1, is_plugin: false, tab_id: 0 }],
    spawned,
    ready(),
    completed(),
  ], { control: { session: "dev", paneId: "terminal_1" }, stackWorkers: true });
  await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  const args = h.calls.find((c) => c.args.includes("spawn"))!.args;
  equal(args[args.indexOf("--stack-for") + 1], "terminal_1");
  assert(!args.includes("--tab"));
});

Deno.test("cleanup preserves evidence and retries guarded close failures", async () => {
  const h = harness([spawned, ready(), completed()]);
  await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  const result = structuredClone(h.records.get("result"));
  h.replies.push([{ id: 7, is_plugin: false }], { outcome: "target_changed" });
  await rejects(
    () => cleanupAgent(h.context, h.dependencies),
    "cleanup blocked",
  );
  assert(!h.records.get("state")!.cleanedUp);
  equal(h.records.get("result"), result);
  h.replies.push([{ id: 7, is_plugin: false }], { outcome: "ok" });
  await cleanupAgent(h.context, h.dependencies);
  const args = h.calls.at(-1)!.args;
  equal(args[args.indexOf("--submission") + 1], "submission-1");
  equal(args[args.indexOf("--conversation") + 1], "conversation-1");
  const count = h.calls.length;
  await cleanupAgent(h.context, h.dependencies);
  equal(h.calls.length, count);
});

Deno.test("cleanup reconciles an already absent worker without closing another pane", async () => {
  const h = harness([spawned, ready(), completed(), [{
    id: 8,
    is_plugin: false,
  }]]);
  await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  await cleanupAgent(h.context, h.dependencies);
  assert(h.records.get("state")!.cleanedUp);
  assert(!h.calls.some((c) => c.args.includes("close-completed")));
});

Deno.test("stack placement never silently falls back without a controller", async () => {
  const h = harness([], { stackWorkers: true });
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "require a control pane",
  );
  equal(h.calls.length, 0);
});

Deno.test("submission continuation retains the original conversation and recovers a lost acknowledgement without another prompt", async () => {
  const h = harness([spawned, ready(), completed()]);
  await runAgent(
    { prompt: "Original investigation" },
    h.context,
    h.dependencies,
  );
  const origin = AgentStateSchema.parse(h.records.get("state"));
  h.dependencies.id = () => "submission-2";
  h.replies.push(ready(), new Error("Lost continuation acknowledgement"));
  const args = {
    id: "a6da10b0-c980-4b82-b920-000000000001",
    prompt: "Submit the existing findings through Swamp",
  };
  await rejects(
    () => continueAgent(args, h.context, h.dependencies),
    "Lost continuation acknowledgement",
  );
  const pending = AgentStateSchema.parse(h.records.get("state"));
  equal(pending.prompt, origin.prompt);
  equal(pending.promptDigest, origin.promptDigest);
  equal(pending.continuation?.previousSubmissionId, origin.submissionId);
  equal(pending.submissionId, "submission-2");
  h.replies.push(completed({ submission_id: "submission-2", message: null }));
  await continueAgent(args, h.context, h.dependencies);
  equal(h.calls.filter((c) => c.args.includes("spawn")).length, 1);
  equal(h.calls.filter((c) => c.args.includes("prompt")).length, 2);
  equal(
    h.calls.at(-1)?.args[h.calls.at(-1)!.args.indexOf("--submission") + 1],
    "submission-2",
  );
  equal(h.records.get("result")?.message, undefined);
  await continueAgent(args, h.context, h.dependencies);
  equal(h.calls.filter((c) => c.args.includes("prompt")).length, 2);
});

Deno.test("delayed continuation retries cannot repeat provider work or overwrite turn history", async () => {
  const h = harness([spawned, ready(), completed()]);
  await runAgent({ prompt: "Investigate" }, h.context, h.dependencies);
  const turns = [1, 2, 3].map((index) => ({
    id: `a6da10b0-c980-4b82-b920-00000000000${index}`,
    prompt: `Submit established findings ${index}`,
  }));
  for (const [index, turn] of turns.entries()) {
    const submission = `submission-${index + 2}`;
    h.dependencies.id = () => submission;
    h.replies.push(ready(), completed({ submission_id: submission }));
    await continueAgent(turn, h.context, h.dependencies);
  }
  const records = structuredClone([...h.records]);
  const calls = h.calls.length;
  const writes = h.writes.length;
  for (const turn of turns.slice(0, -1)) {
    await rejects(
      () => continueAgent(turn, h.context, h.dependencies),
      "already used by an older turn",
    );
    await rejects(
      () =>
        continueAgent(
          { ...turn, prompt: "Changed request" },
          h.context,
          h.dependencies,
        ),
      "different prompt",
    );
  }
  equal([...h.records], records);
  equal(h.calls.length, calls);
  equal(h.writes.length, writes);
  await continueAgent(turns[2], h.context, h.dependencies);
  equal(h.calls.length, calls);
});

Deno.test("continuation fails closed on incomplete or cyclic history and existing snapshots", async () => {
  for (const fault of ["missing", "malformed", "cycle", "existing"]) {
    const h = harness([spawned, ready(), completed()]);
    await runAgent({ prompt: "Investigate" }, h.context, h.dependencies);
    const first = {
      id: "a6da10b0-c980-4b82-b920-000000000001",
      prompt: "Submit findings",
    };
    h.dependencies.id = () => "submission-2";
    h.replies.push(ready(), completed({ submission_id: "submission-2" }));
    await continueAgent(first, h.context, h.dependencies);
    const next = { ...first, id: "a6da10b0-c980-4b82-b920-000000000002" };
    const key = `turn-${first.id}`;
    if (fault === "missing") h.records.delete(key);
    if (fault === "malformed") h.records.set(key, { phase: "completed" });
    if (fault === "cycle") {
      h.records.set(key, structuredClone(h.records.get("state")!));
    }
    if (fault === "existing") {
      h.records.set(`turn-${next.id}`, structuredClone(h.records.get(key)!));
    }
    const records = structuredClone([...h.records]);
    const calls = h.calls.length;
    const writes = h.writes.length;
    await rejects(
      () => continueAgent(next, h.context, h.dependencies),
      fault === "malformed"
        ? "invalid_type"
        : fault === "existing"
        ? "snapshot already exists"
        : fault === "cycle"
        ? "history contains a cycle"
        : "history is missing",
    );
    equal([...h.records], records);
    equal(h.calls.length, calls);
    equal(h.writes.length, writes);
  }
});

Deno.test("continuation refuses active, cleaned or replacement conversations without launching another worker", async () => {
  const args = {
    id: "a6da10b0-c980-4b82-b920-000000000001",
    prompt: "Submit existing evidence",
  };
  for (const scenario of ["active", "cleaned", "replacement"] as const) {
    const h = harness([spawned, ready(), completed()]);
    await runAgent({ prompt: "Investigate" }, h.context, h.dependencies);
    const state = AgentStateSchema.parse(h.records.get("state"));
    if (scenario === "active") state.phase = "waiting";
    if (scenario === "cleaned") state.cleanedUp = true;
    h.records.set("state", state);
    if (scenario === "replacement") h.replies.push(ready("replacement"));
    await rejects(
      () => continueAgent(args, h.context, h.dependencies),
      scenario === "replacement"
        ? "conversation changed"
        : "original completed",
    );
    equal(h.calls.filter((c) => c.args.includes("spawn")).length, 1);
    equal(h.calls.filter((c) => c.args.includes("prompt")).length, 1);
  }
});

Deno.test("a different continuation cannot replace a timed-out turn's saved identity or prompt", async () => {
  const h = harness([spawned, ready(), completed()]);
  await runAgent({ prompt: "Investigate" }, h.context, h.dependencies);
  h.dependencies.id = () => "submission-2";
  const args = {
    id: "a6da10b0-c980-4b82-b920-000000000001",
    prompt: "Submit the original findings",
  };
  h.replies.push(
    ready(),
    completed({
      outcome: "timed_out",
      submission_id: "submission-2",
      acknowledged: true,
    }),
  );
  await rejects(
    () => continueAgent(args, h.context, h.dependencies),
    "timed_out",
  );
  const pending = structuredClone(h.records.get("state"));
  const writes = h.writes.length;
  const calls = h.calls.length;
  await rejects(
    () =>
      continueAgent(
        {
          id: "a6da10b0-c980-4b82-b920-000000000002",
          prompt: "Different request",
        },
        h.context,
        h.dependencies,
      ),
    "original completed, retained conversation",
  );
  equal(h.records.get("state"), pending);
  equal(h.writes.length, writes);
  equal(h.calls.length, calls);
  h.replies.push(completed({ submission_id: "submission-2" }));
  await continueAgent(args, h.context, h.dependencies);
  equal(h.records.get("result")?.submissionId, "submission-2");
  equal(h.calls.filter((c) => c.args.includes("prompt")).length, 2);
  equal(h.calls.filter((c) => c.args.includes("spawn")).length, 1);
});

Deno.test("launch transaction resumes the original started operation after a lost reply without spawning twice", async () => {
  const h = harness([
    new Error("lost reply"),
    { ...spawned, launch: receipt("server-test.submission-1") },
    ready(),
    completed(),
  ]);
  await rejects(
    () =>
      runAgent({ prompt: "Original approved task" }, h.context, h.dependencies),
    "lost reply",
  );
  const intent = AgentStateSchema.parse(h.records.get("state"));
  equal(intent.phase, "launching");
  equal(intent.launchId, "server-test.submission-1");
  equal(intent.submissionId, undefined);
  await resumeAgent(h.context, h.dependencies);
  equal(h.calls.map((call) => call.args[3]), [
    "spawn",
    "launch-status",
    "wait",
    "prompt",
  ]);
  equal(h.calls[3].stdin, "Original approved task");
  equal(h.records.get("result")?.promptDigest, intent.promptDigest);
});

Deno.test("launch transaction pending and unknown observations retain exact intent and never submit", async () => {
  const h = harness([
    {
      outcome: "status_unavailable",
      launch: receipt("server-test.submission-1", {
        outcome: "pending",
        phase: "prepares",
        process: "not_started",
        disposition: "reserved",
      }),
    },
    {
      outcome: "status_unavailable",
      launch: receipt("server-test.submission-1", {
        outcome: "unknown",
        process: "unknown",
        disposition: "unknown",
      }),
    },
  ]);
  await rejects(
    () => runAgent({ prompt: "Original task" }, h.context, h.dependencies),
    "Launch pending",
  );
  await rejects(() => resumeAgent(h.context, h.dependencies), "Launch unknown");
  const state = AgentStateSchema.parse(h.records.get("state"));
  equal(state.launchId, "server-test.submission-1");
  equal(state.prompt, "Original task");
  equal(state.submissionId, undefined);
  equal(h.calls.map((call) => call.args[3]), ["spawn", "launch-status"]);
});

Deno.test("launch transaction refuses a changed digest or server on observation", async () => {
  for (
    const changed of [{ request_digest: "b".repeat(64) }, {
      server_instance: "replacement",
    }]
  ) {
    const h = harness([
      {
        outcome: "status_unavailable",
        launch: receipt("server-test.submission-1", {
          outcome: "pending",
          process: "not_started",
          disposition: "reserved",
        }),
      },
      { ...spawned, launch: receipt("server-test.submission-1", changed) },
    ]);
    await rejects(
      () => runAgent({ prompt: "Original task" }, h.context, h.dependencies),
      "Launch pending",
    );
    await rejects(
      () => resumeAgent(h.context, h.dependencies),
      "does not match",
    );
    equal(h.records.get("state")?.launchRequestDigest, "a".repeat(64));
    equal(h.records.get("state")?.submissionId, undefined);
    equal(h.calls.filter((call) => call.args[3] === "spawn").length, 1);
  }
});
