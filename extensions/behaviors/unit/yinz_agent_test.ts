import {
  AgentConfigSchema,
  type AgentContext,
  AgentStateSchema,
  type CommandResult,
  confirmFailureAgent,
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
type Reply = Record<string, unknown> | Error;

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
      calls.push({
        args,
        stdin,
        signal,
        state: structuredClone(records.get("state")),
      });
      const reply = replies.shift();
      if (reply instanceof Error) return Promise.reject(reply);
      if (!reply) throw new Error(`Unexpected command: ${args.join(" ")}`);
      if ("code" in reply) return Promise.resolve(reply as CommandResult);
      return Promise.resolve({
        code: 0,
        stdout: JSON.stringify(reply),
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
  const h = harness([new Error("lost spawn response")]);
  await rejects(
    () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
    "lost spawn response",
  );
  await rejects(
    () => resumeAgent(h.context, h.dependencies),
    "Launch outcome is uncertain",
  );
  equal(h.calls.length, 1);
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
    equal(JSON.parse(state.rawResponse!), rejected);
    equal(state.paneId, undefined);
    equal(state.submissionId, undefined);
    equal(
      h.writes.find(({ value }) => value.rawResponse)?.value.phase,
      "launch_rejected",
    );
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
    const h = harness([reply]);
    await rejects(
      () => runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
      "",
    );
    equal(h.records.get("state")?.phase, "launching");
    assert(!h.records.has("result"));
    await rejects(
      () => resumeAgent(h.context, h.dependencies),
      "Launch outcome is uncertain",
    );
    equal(h.calls.length, 1);
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

Deno.test("completion requires matching submission, unchanged conversation and a final message", async () => {
  for (
    const [value, error] of [
      [completed({ submission_id: "someone-else" }), "saved submission"],
      [completed({ agents: [agent("replacement")] }), "conversation changed"],
      [completed({ message: null }), "final message"],
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

Deno.test("missing final text persists terminal failure without reusing stale text or retrieving again", async () => {
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
      await rejects(
        () =>
          recover
            ? resumeAgent(h.context, h.dependencies)
            : runAgent({ prompt: "Fix it" }, h.context, h.dependencies),
        "discard-agent",
      );
      const state = AgentStateSchema.parse(h.records.get("state"));
      equal(state.phase, "completed_unusable");
      equal(state.outcome, "completed");
      equal(state.message, undefined);
      equal(state.backgroundTasks, []);
      assert(!h.records.has("result"));
      const count = h.calls.length;
      await rejects(
        () => resumeAgent(h.context, h.dependencies),
        "discard-agent",
      );
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
  const h = harness([
    spawned,
    ready(),
    completed({ background_tasks: ["worker-1"], message: "Working" }),
    { ...ready(), message: "Final answer" },
  ]);
  await runAgent({ prompt: "Fix it" }, h.context, h.dependencies);
  equal(h.calls[3].state?.phase, "background");
  assert(h.calls[3].args.includes("--until"));
  assert(!h.calls[3].args.includes("--submission"));
  equal(h.records.get("result")?.message, "Final answer");
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
  const failure = completed({ outcome: "failed" });
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
  const failure = completed({ outcome: "failed" });
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
