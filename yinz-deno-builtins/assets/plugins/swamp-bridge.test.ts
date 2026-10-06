import { test, loadPlugin, assert } from "yinz:testing";
test("swamp-bridge rejects unauthenticated callers before dispatch", async () => {
  const p = await loadPlugin("./swamp-bridge.ts", {
    replies: {
      GetPluginIds: { host_id: "test", plugin_id: 1 },
      DataDefineEntity: null,
      DataFetch: { columns: [], rows: [], truncated: false, next_cursor: null },
      MessageToPlugin: null,
      GetPaneInfo: { plugin_url: "file:untrusted.ts" },
    },
  });
  await p.pipe({
    name: "yinz_rpc_call",
    source: { Plugin: 2 },
    payload: JSON.stringify({
      version: 1,
      call_id: "a",
      name: "swamp",
      payload: { method: "workflow_run", work: "x", key: "one" },
    }),
  });
  assert(JSON.stringify(p.commands).includes("Only builtin workflow views"));
  assert(!JSON.stringify(p.commands).includes("SwampRequest"));
});
import { assertEquals } from "yinz:testing";
import { storeReplies } from "./testing-fixtures/store.ts";
import type { AgentEvent } from "yinz:plugin";

async function promptFixture(
  legacy = false,
  prompt_sha256?: string,
  kind = legacy ? "claude" : "fixture",
) {
  const f = await cleanupFixture();
  const owner = { ...f.identity, work: "work", agent: "agent" };
  f.seed("work", "agent", {
    request: JSON.stringify({ method: "agent_spawn" }),
    owner,
    pane: 9,
    supervisor: "supervisor",
    process: 42,
    ready: true,
    outcome: "completed",
  });
  f.p.stub("GetPaneInfo", (pane: { Terminal?: number }) =>
    pane.Terminal == null ? { plugin_url: "yinz:agent-exit" } : { agent: { kind } },
  );
  f.p.stub("GetAgentProviders", [
    { kind: "fixture", executable: "/opt/bin/fixture-cli", aliases: ["fixture-alias"], input: {} },
    { kind: "opencode", executable: "opencode" },
    { kind: "no-input", executable: "no-input" },
  ]);
  f.p.stub("AgentControl", null);
  f.p.stub("WriteToPaneId", null);
  const payload = {
    method: "agent_prompt",
    work: "work",
    agent: "agent",
    key: "prompt",
    prompt: "first\nsecond",
    ...(prompt_sha256 ? { prompt_sha256 } : {}),
  };
  const pending = await f.request(payload);
  await f.hostReply([
    {
      terminal_id: 9,
      host_id: "supervisor",
      child_pid: 42,
      attached: true,
      session_name: f.identity.session,
    },
  ]);
  function saved(key = "prompt") {
    return JSON.parse(
      f.store.rows.get(JSON.stringify([f.identity.host_id, f.identity.session, "work", key]))!
        .value,
    );
  }
  async function hook(hook: string, fields: Partial<AgentEvent> = {}, raw = {}) {
    await f.p.event({
      AgentEvent: {
        pane_id: 9,
        kind: "fixture",
        hook,
        session_id: "agent-session",
        state: "Idle",
        detail: null,
        prompt: payload.prompt,
        last_message: "done",
        tool_name: null,
        cwd: null,
        raw: JSON.stringify({
          _yinz_workflow_owner: JSON.stringify(owner),
          turn_id: "turn",
          ...raw,
        }),
        ...fields,
      },
    });
  }
  async function response(fields = {}) {
    const command = (f.p.commands as { AgentControl?: { id: string } }[]).find(
      (entry) => entry.AgentControl,
    )!.AgentControl!;
    await f.p.event({
      CustomMessage: [
        "agent-response",
        JSON.stringify({
          id: command.id,
          response: {
            outcome: "submitted",
            acknowledged: true,
            agents: [{ agent: { session_id: "agent-session" } }],
            turn_id: "turn",
            ...fields,
          },
        }),
      ],
    });
  }
  return { ...f, owner, payload, pending, saved, hook, response };
}

for (const kind of [
  "fixture",
  "/opt/bin/FIXTURE-CLI",
  "FIXTURE-ALIAS",
  "opencode",
  "no-input",
  "claude",
  "unknown",
]) {
  test(`prompt input follows registered capabilities: ${kind}`, async () => {
    const f = await promptFixture(false, undefined, kind);
    const guarded = kind.toLowerCase().includes("fixture");
    const controls = (f.p.commands as { AgentControl?: { request: string } }[]).filter(
      (entry) => entry.AgentControl,
    );
    assertEquals(controls.length, guarded ? 1 : 0);
    const writes = (f.p.commands as { WriteToPaneId?: [number[], unknown] }[]).filter(
      (entry) => entry.WriteToPaneId,
    );
    if (guarded) {
      assertEquals(JSON.parse(controls[0]!.AgentControl!.request).Prompt.wait, false);
      assertEquals(writes.length, 0);
      await f.response();
    } else {
      assertEquals(
        writes.map((entry) => String.fromCharCode(...entry.WriteToPaneId![0])),
        ["\x1b[200~first\nsecond\x1b[201~", "\r"],
      );
    }
    await f.hook("UserPromptSubmit");
    await f.hook("Stop");
    assertEquals(f.saved().outcome, "completed");
  });
}

for (const prompt of ["provider prefix: first\nsecond", "first\r\nsecond"]) {
  for (const hashed of [false, true]) {
    test(`guarded prompt completes after transformation: ${JSON.stringify(prompt)}, hashes: ${hashed}`, async () => {
      const f = await promptFixture(false, hashed ? "request-hash" : undefined);
      await f.hook("UserPromptSubmit", { prompt }, hashed ? { prompt_sha256: "hook-hash" } : {});
      assert(!f.saved().accepted);
      await f.response();
      assertEquals(f.saved().agentSession, "agent-session");
      assertEquals(f.saved().turn, "turn");
      await f.hook("Stop");
      assertEquals(f.saved().outcome, "completed");
      assertEquals(f.saved("agent").outstanding, undefined);
    });
  }
}

for (const earlyCompletion of [false, true]) {
  test(`guarded prompt replays only matching completion hooks, early completion: ${earlyCompletion}`, async () => {
    const f = await promptFixture();
    await f.hook("Stop", { session_id: "unrelated" });
    await f.hook("Stop", {}, { turn_id: "unrelated" });
    await f.hook("Stop", { pane_id: 10 });
    await f.hook(
      "Stop",
      {},
      {
        _yinz_workflow_owner: JSON.stringify({ ...f.owner, incarnation: "unrelated" }),
      },
    );
    if (earlyCompletion) await f.hook("Stop");
    await f.response();
    assertEquals(f.saved().outcome, earlyCompletion ? "completed" : "pending");
    if (!earlyCompletion) {
      await f.hook("Stop", { session_id: "unrelated" });
      await f.hook("Stop", {}, { turn_id: "unrelated" });
      assertEquals(f.saved().outcome, "pending");
      await f.hook("Stop");
    }
    assertEquals(f.saved().outcome, "completed");
    assertEquals(f.saved("agent").outstanding, undefined);
  });
}

for (const missingAcknowledgement of [false, true]) {
  test(`status unavailable reserves the prompt slot, missing acknowledgement: ${missingAcknowledgement}`, async () => {
    const f = await promptFixture();
    await f.hook("UserPromptSubmit", { prompt: "provider prefix" });
    await f.response({
      outcome: "status_unavailable",
      ...(missingAcknowledgement
        ? { acknowledged: undefined, agents: [], turn_id: undefined }
        : {}),
    });
    assertEquals(f.saved().outcome, "unknown");
    assert(f.saved().dispatched);
    assert(f.saved("agent").outstanding);
    assert(f.pending.reply().error.includes("status_unavailable"));
    assertEquals((await f.request(f.payload)).reply().result.outcome, "unknown");
    await f.hook("SessionStart");
    await f.hostReply([
      {
        terminal_id: 9,
        host_id: "supervisor",
        child_pid: 42,
        attached: true,
        session_name: f.identity.session,
      },
    ]);
    assertEquals(
      (f.p.commands as { AgentControl?: unknown }[]).filter((entry) => entry.AgentControl).length,
      1,
    );
    assertEquals(f.saved().outcome, "unknown");
    assert(f.saved("agent").outstanding);
  });
}

test("explicit prompt rejection fails and releases the slot", async () => {
  const f = await promptFixture();
  await f.response({ outcome: "rejected", acknowledged: false, agents: [], turn_id: null });
  assertEquals(f.saved().outcome, "failed");
  assertEquals(f.saved("agent").outstanding, undefined);
});

for (const hashed of [false, true]) {
  test(`legacy prompt completes through matching hooks, hashes: ${hashed}`, async () => {
    const f = await promptFixture(true, hashed ? "same-hash" : undefined);
    await f.hook("UserPromptSubmit", { prompt: "unrelated" }, { prompt_sha256: "wrong-hash" });
    await f.hook("Stop");
    assertEquals(f.saved().outcome, "pending");
    await f.hook(
      "UserPromptSubmit",
      hashed ? { prompt: "transformed" } : {},
      hashed ? { prompt_sha256: "same-hash" } : {},
    );
    await f.hook("Stop");
    assertEquals(f.saved().outcome, "completed");
    assertEquals(f.saved("agent").outstanding, undefined);
  });
}

test("late submission response preserves a terminal prompt outcome", async () => {
  const f = await promptFixture();
  await f.hook("UserPromptSubmit");
  await f.hook("StopFailure");
  const saved = f.saved();
  await f.response();
  assertEquals(f.saved(), saved);
  assertEquals(f.saved().outcome, "failed");
});

test("CLI workflow operations use the shared validator and correlated one-shot replies", async () => {
  const f = await cleanupFixture();
  f.p.stub("BlockCliPipeInput", null);
  f.p.stub("CliPipeOutput", null);
  f.p.stub("UnblockCliPipeInput", null);
  const payload = JSON.stringify({
    tool: "workflow_search",
    directory: "/repo",
    args: { query: "fixture" },
  });
  const before = f.p.commands.length;
  await f.p.pipe({ name: "workflow_action", source: { Plugin: 99 }, payload });
  assertEquals(f.p.commands.length, before, "plugins cannot impersonate CLI callers");
  await f.p.pipe({ name: "workflow_action", source: { Cli: "query-1" }, payload });
  await f.swampReply("workflow.search", { results: [{ name: "fixture" }] });
  const replies = () =>
    (f.p.commands as { CliPipeOutput?: [string, string] }[]).flatMap((c) =>
      c.CliPipeOutput ? [c.CliPipeOutput] : [],
    );
  assertEquals(replies().length, 1);
  assertEquals(replies()[0]![0], "query-1");
  const reply = JSON.parse(replies()[0]![1]);
  assertEquals(reply.ok, true);
  assertEquals(reply.data, { results: [{ name: "fixture" }] });
  await f.p.pipe({ name: "workflow_action", source: { Cli: "bad-json" }, payload: "{" });
  assertEquals(JSON.parse(replies().at(-1)![1]).ok, false);
  assert(JSON.stringify(f.p.commands).includes('"UnblockCliPipeInput":"bad-json"'));
});

test("swamp-bridge correlates persisted attachments and rejects conflicting duplicates", async () => {
  const store = storeReplies();
  const p = await loadPlugin("./swamp-bridge.ts", {
    replies: {
      ...store.replies,
      GetPluginIds: { host_id: "test", plugin_id: 1 },
      MessageToPlugin: null,
      GetPaneInfo: { plugin_url: "yinz:swamp-workflow" },
    },
  });
  let id = 0;
  async function request(payload: unknown) {
    const call_id = String(++id);
    await p.pipe({
      name: "yinz_rpc_call",
      source: { Plugin: 2 },
      payload: JSON.stringify({ version: 1, call_id, name: "swamp", payload }),
    });
    const commands = p.commands as {
      MessageToPlugin?: { message_name: string; message_payload: string };
    }[];
    const reply = commands
      .filter((c) => c.MessageToPlugin?.message_name === "yinz_rpc_reply")
      .at(-1)!.MessageToPlugin!;
    const result = JSON.parse(reply.message_payload);
    assertEquals(result.call_id, call_id);
    return result;
  }
  const attach = {
    method: "workflow_attach",
    run: "run-1",
    work: "work-1",
    key: "attach",
    workflow: "fixture",
    directory: "/fixture",
  };
  assertEquals((await request(attach)).result.outcome, "completed");
  assertEquals(store.rows.size, 2);
  const saved = JSON.stringify([...store.rows]);
  assertEquals((await request(attach)).result.outcome, "completed");
  assertEquals(JSON.stringify([...store.rows]), saved);
  assert(
    (await request({ ...attach, work: "other" })).error.includes(
      "Run belongs to a different launch",
    ),
  );
  assert(
    (await request({ method: "lookup", work: "bad key", key: "k" })).error.includes(
      "Invalid correlation key",
    ),
  );
});

async function cleanupFixture() {
  const store = storeReplies();
  const p = await loadPlugin("./swamp-bridge.ts", {
    replies: {
      ...store.replies,
      GetPluginIds: { host_id: "test", plugin_id: 1 },
      MessageToPlugin: null,
      GetPaneInfo: { plugin_url: "yinz:agent-exit" },
      SetPaneSubtitle: null,
      RunCommand: null,
      SwampRequest: null,
      CloseTerminalPane: null,
      OpenCommandPaneBackground: { Terminal: 9 },
      ProjectConfigRead: { root: "/repo", config: { workflow: "fixture" } },
    },
  });
  let id = 0;
  async function request(payload: unknown, source = 2, name = "swamp") {
    const call_id = String(++id);
    await p.pipe({
      name: "yinz_rpc_call",
      source: { Plugin: source },
      payload: JSON.stringify({ version: 1, call_id, name, payload }),
    });
    return {
      call_id,
      reply: () => {
        const messages = p.commands as {
          MessageToPlugin?: { message_name: string; message_payload: string };
        }[];
        return messages
          .filter((entry) => entry.MessageToPlugin?.message_name === "yinz_rpc_reply")
          .map((entry) => JSON.parse(entry.MessageToPlugin!.message_payload))
          .find((entry) => entry.call_id === call_id);
      },
    };
  }
  const identity = (await request({ method: "identity" })).reply().result;
  function seed(work: string, key: string, value: Record<string, unknown>) {
    const uid = JSON.stringify([identity.host_id, identity.session, work, key]);
    store.rows.set(uid, {
      value: JSON.stringify({ work, key, incarnation: identity.incarnation, ...value }),
      rev: 1,
    });
  }
  async function hostReply(rows: unknown[]) {
    const command = [...p.commands]
      .reverse()
      .find((entry) => typeof entry === "object" && entry != null && "RunCommand" in entry) as {
      RunCommand: unknown[];
    };
    assert(command);
    const text = JSON.stringify(rows);
    await p.event({
      RunCommandResult: [
        0,
        [...text].map((c) => c.charCodeAt(0)),
        [],
        command.RunCommand[3] as Record<string, string>,
      ],
    });
  }
  async function swampReply(operation: string, payload: unknown) {
    const commands = p.commands as { SwampRequest?: { id: string; operation: string } }[];
    const command = commands
      .filter((entry) => entry.SwampRequest?.operation === operation)
      .at(-1)?.SwampRequest;
    assert(command, operation);
    await p.event({
      CustomMessage: [
        "swamp-response",
        JSON.stringify({ id: command.id, type: "result", payload }),
      ],
    });
  }
  return { p, store, request, identity, seed, hostReply, swampReply };
}

test("cleanup preparation authenticates the owning exit pane, blocks late launch and permits retries", async () => {
  const f = await cleanupFixture();
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: true,
  });
  assert(
    (await f.request({ method: "cleanup_prepare", work: "exit" }, 3))
      .reply()
      .error.includes("another hook"),
  );
  assert(
    (await f.request({ method: "workflow_run", work: "exit", key: "launch" }))
      .reply()
      .error.includes("lifecycle is closed"),
  );
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: true,
  });
});

test("cleanup refuses an uncertain launch and can retry after ownership is reconciled", async () => {
  const f = await cleanupFixture();
  f.seed("exit", "command", {
    request: JSON.stringify({ method: "command" }),
    owner: { ...f.identity, work: "exit", agent: "command" },
    outcome: "unknown",
  });
  assert(
    (await f.request({ method: "cleanup_prepare", work: "exit" }))
      .reply()
      .error.includes("termination is uncertain"),
  );
  f.seed("exit", "command", {
    request: JSON.stringify({ method: "command" }),
    outcome: "cancelled",
    terminationVerified: true,
  });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: true,
  });
});

for (const outcome of ["pending", "completed", "failed"]) {
  test(`cleanup verifies process-group termination for ${outcome} operations`, async () => {
    const f = await cleanupFixture();
    f.seed("exit", "command", {
      request: JSON.stringify({ method: "command" }),
      outcome,
      owner: { ...f.identity, work: "exit", agent: "command" },
      pane: 9,
      supervisor: "host-9",
      process: 123,
    });
    const first = await f.request({ method: "cleanup_prepare", work: "exit" });
    const duplicate = await f.request({ method: "cleanup_prepare", work: "exit" });
    assert(!first.reply());
    assert(!duplicate.reply());
    assertEquals(closedHelpers(f.p), []);
    await f.hostReply([
      {
        terminal_id: 9,
        host_id: "host-9",
        child_pid: 123,
        session_name: f.identity.session,
        attached: true,
        child_group_terminated: true,
        child_exit_status: 0,
      },
    ]);
    assertEquals(first.reply().result, { ready: true });
    assertEquals(duplicate.reply().result, { ready: true });
    assertEquals(closedHelpers(f.p), [{ CloseTerminalPane: 9 }]);
  });
}

test("cleanup refuses a replacement process without signalling it", async () => {
  const f = await cleanupFixture();
  f.seed("exit", "command", {
    request: JSON.stringify({ method: "command" }),
    outcome: "pending",
    owner: { ...f.identity, work: "exit", agent: "command" },
    pane: 9,
    supervisor: "host-9",
    process: 123,
  });
  const pending = await f.request({ method: "cleanup_prepare", work: "exit" });
  await f.hostReply([
    {
      terminal_id: 9,
      host_id: "host-9",
      child_pid: 456,
      session_name: f.identity.session,
      attached: true,
    },
  ]);
  assert(pending.reply().error.includes("ownership changed"));
  assertEquals(closedHelpers(f.p), []);
});

test("cleanup waits for a command launch to publish supervisor identity", async () => {
  const f = await cleanupFixture();
  const launch = await f.request({
    method: "command",
    work: "exit",
    key: "command",
    argv: ["helper"],
    cwd: "/repo",
  });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: false,
  });
  const host = {
    terminal_id: 9,
    host_id: "host-9",
    child_pid: 123,
    session_name: f.identity.session,
    attached: true,
  };
  await f.hostReply([host]);
  assert(launch.reply().error.includes("lifecycle is closed"));
  const cleanup = await f.request({ method: "cleanup_prepare", work: "exit" });
  assert(!cleanup.reply());
  await f.hostReply([{ ...host, child_group_terminated: true }]);
  assertEquals(cleanup.reply().result, { ready: true });
});

test("cleanup blocks a workflow launch still validating", async () => {
  const f = await cleanupFixture();
  const launch = await f.request({
    method: "workflow_run",
    work: "exit",
    key: "launch",
    directory: "/repo",
    exactInputs: true,
  });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: false,
  });
  await f.swampReply("workflow.get", { name: "fixture" });
  await f.swampReply("workflow.validate", { passed: true });
  assert(launch.reply().error.includes("lifecycle is closed"));
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: true,
  });
  assert(!JSON.stringify(f.p.commands).includes('"operation":"workflow.run"'));
});

for (const failure of ["cancel failed", "still running", "none"]) {
  test(`cleanup waits for authoritative workflow cancellation: ${failure}`, async () => {
    const f = await cleanupFixture();
    f.seed("exit", "launch", {
      request: JSON.stringify({ method: "workflow_run" }),
      checkout: "/repo",
      run: "run-1",
      outcome: "pending",
    });
    const cleanup = await f.request({ method: "cleanup_prepare", work: "exit" });
    await f.swampReply("workflow.history.get", { status: "running" });
    assert(!cleanup.reply());
    await f.swampReply(
      "workflow.cancel",
      failure === "cancel failed" ? { error: { code: "unavailable" } } : {},
    );
    if (failure !== "cancel failed") {
      assert(!cleanup.reply());
      await f.swampReply("workflow.history.get", {
        status: failure === "none" ? "cancelled" : "running",
      });
    }
    if (failure === "none") assertEquals(cleanup.reply().result, { ready: true });
    else {
      assert(cleanup.reply().error);
      const retry = await f.request({ method: "cleanup_prepare", work: "exit" });
      await f.swampReply("workflow.history.get", { status: "cancelled" });
      assertEquals(retry.reply().result, { ready: true });
    }
  });
}

for (const status of ["succeeded", "suspended", "failed", "cancelled"]) {
  test(`cleanup accepts a settled ${status} workflow after closing its lifecycle`, async () => {
    const f = await cleanupFixture();
    f.seed("exit", "launch", {
      request: JSON.stringify({ method: "workflow_run" }),
      checkout: "/repo",
      run: "run-1",
      outcome: "completed",
    });
    const cleanup = await f.request({ method: "cleanup_prepare", work: "exit" });
    await f.swampReply("workflow.history.get", { status });
    assertEquals(cleanup.reply().result, { ready: true });
  });
}

test("cleanup cancels a workflow whose dispatch acknowledgement arrives late", async () => {
  const f = await cleanupFixture();
  const launch = await f.request({
    method: "workflow_run",
    work: "exit",
    key: "launch",
    directory: "/repo",
    exactInputs: true,
  });
  await f.swampReply("workflow.get", { name: "fixture" });
  await f.swampReply("workflow.validate", { passed: true });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: false,
  });
  await f.swampReply("workflow.history.search", { results: [] });
  await f.swampReply("workflow.run", { runId: "late-run" });
  assert(launch.reply().error.includes("lifecycle is closed"));
  await f.swampReply("workflow.history.get", { status: "running" });
  await f.swampReply("workflow.cancel", {});
  await f.swampReply("workflow.history.get", { status: "cancelled" });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: true,
  });
});

test("cleanup waits for an answer already submitting and recancels its resumed run", async () => {
  const f = await cleanupFixture();
  f.seed("exit", "launch", {
    request: JSON.stringify({ method: "workflow_run" }),
    checkout: "/repo",
    run: "run-1",
    outcome: "completed",
  });
  f.seed("run-1", "attachment", { work: "exit", root: "/repo", run: "run-1" });
  const answer = await f.request({
    method: "workflow_answer",
    directory: "/repo",
    model: "question",
    inputs: { run: "run-1", question: "q", answer: "continue" },
  });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: false,
  });
  await f.swampReply("workflow.history.get", { status: "suspended" });
  await f.swampReply("model.method.run", {});
  assert(answer.reply().result);
  await f.swampReply("workflow.history.get", { status: "running" });
  await f.swampReply("workflow.cancel", {});
  await f.swampReply("workflow.history.get", { status: "cancelled" });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: true,
  });
});

test("answer recovery uses the exact saved question without submitting a new answer", async () => {
  const f = await cleanupFixture();
  f.seed("run-1", "attachment", { work: "exit", root: "/repo", run: "run-1" });
  const recovery = await f.request({
    method: "workflow_recover",
    directory: "/repo",
    model: "run-1-question",
    inputs: { run: "run-1", question: "merge", answer: "ignored", submission: "ignored" },
  });
  await f.swampReply("model.get", { type: "@hammz/question" });
  const commands = f.p.commands as {
    SwampRequest?: { id: string; operation: string; payload: string };
  }[];
  const query = commands
    .filter((entry) => entry.SwampRequest?.operation === "data.query")
    .at(-1)!.SwampRequest!;
  assertEquals(JSON.parse(query.payload), {
    predicate:
      'modelName == "run-1-question" && specName == "question" && attributes.run == "run-1" && attributes.question == "merge"',
    select: '{"attributes":attributes}',
  });
  await f.swampReply("data.query", {
    results: [
      {
        attributes: {
          run: "run-1",
          question: "merge",
          state: "recorded",
          answer: "merge",
          submission: "original",
        },
      },
    ],
  });
  const method = (f.p.commands as typeof commands)
    .filter((entry) => entry.SwampRequest?.operation === "model.method.run")
    .at(-1)!.SwampRequest!;
  assertEquals(JSON.parse(method.payload), {
    modelIdOrName: "run-1-question",
    methodName: "recover",
    inputs: { run: "run-1", question: "merge" },
  });
  await f.p.event({
    CustomMessage: [
      "swamp-response",
      JSON.stringify({
        id: method.id,
        type: "managed.progress",
        payload: { status: "recovering" },
      }),
    ],
  });
  assert(JSON.stringify(f.p.commands).includes("yinz_rpc_progress"));
  await f.swampReply("model.method.run", { run: { status: "succeeded" } });
  assertEquals(recovery.reply().result, { status: "succeeded" });
});

for (const failure of [
  "wrong checkout",
  "wrong model",
  "wrong question",
  "closed",
  "closed during lookup",
]) {
  test(`answer recovery refuses ${failure}`, async () => {
    const f = await cleanupFixture();
    f.seed("run-1", "attachment", {
      work: "exit",
      root: failure === "wrong checkout" ? "/other" : "/repo",
      run: "run-1",
    });
    if (failure === "closed") f.seed("exit", "view", { closed: true });
    const recovery = await f.request({
      method: "workflow_recover",
      directory: "/repo",
      model: "question",
      inputs: { run: "run-1", question: "merge" },
    });
    if (!["closed", "wrong checkout"].includes(failure)) {
      await f.swampReply("model.get", {
        type: failure === "wrong model" ? "@swamp/git" : "@hammz/question",
      });
      if (failure !== "wrong model") {
        if (failure === "closed during lookup") f.seed("exit", "view", { closed: true });
        await f.swampReply("data.query", {
          results: [
            {
              attributes: {
                run: "run-1",
                question: failure === "wrong question" ? "different" : "merge",
              },
            },
          ],
        });
      }
    }
    assert(recovery.reply().error);
    assert(!JSON.stringify(f.p.commands).includes('"operation":"model.method.run"'));
  });
}

test("cleanup waits for an answer recovery already dispatched", async () => {
  const f = await cleanupFixture();
  f.seed("exit", "launch", {
    request: JSON.stringify({ method: "workflow_run" }),
    checkout: "/repo",
    run: "run-1",
    outcome: "completed",
  });
  f.seed("run-1", "attachment", { work: "exit", root: "/repo", run: "run-1" });
  const recovery = await f.request({
    method: "workflow_recover",
    directory: "/repo",
    model: "question",
    inputs: { run: "run-1", question: "merge" },
  });
  await f.swampReply("model.get", { type: "@hammz/question" });
  await f.swampReply("data.query", {
    results: [{ attributes: { run: "run-1", question: "merge" } }],
  });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: false,
  });
  await f.swampReply("workflow.history.get", { status: "suspended" });
  await f.swampReply("model.method.run", {});
  assert(recovery.reply().result);
  await f.swampReply("workflow.history.get", { status: "running" });
  await f.swampReply("workflow.cancel", {});
  await f.swampReply("workflow.history.get", { status: "cancelled" });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: true,
  });
});

for (const status of ["succeeded", "failed", "cancelled", "suspended", "running", "unknown"]) {
  test(`workflow cancellation reports the exact ${status} execution truthfully`, async () => {
    const f = await cleanupFixture();
    const attachment = { work: "exit", root: "/repo", run: "run-1", workflow: "fixture" };
    f.seed("exit", "launch", { checkout: "/repo", not_dispatched: true, request: "{}" });
    f.seed("run-1", "attachment", attachment);
    f.seed("exit", "runs", { entries: { "run-1": attachment } });
    const cancellation = await f.request({
      method: "workflow_cancel",
      directory: "/repo",
      run: "run-1",
    });
    await f.swampReply("workflow.history.get", {
      id: "run-1",
      status,
      inputs: { session: f.identity.session, launch: "exit" },
    });
    await f.swampReply("data.query", { results: [] });
    const commands = f.p.commands as { SwampRequest?: { operation: string; payload: string } }[];
    const outputs = commands
      .filter((entry) => entry.SwampRequest?.operation === "data.query")
      .at(-1)!.SwampRequest!;
    const projection = JSON.parse(outputs.payload).select as string;
    assert(projection.length <= 4096, `Projection exceeds Swamp limit: ${projection.length}`);
    for (const field of ["reason", "recommendation", "repairReview", "repairTests", "evidence"])
      assert(projection.includes('"' + field + '"'));
    await f.swampReply("data.query", { results: [] });
    await f.swampReply("workflow.history.logs", { log: { lines: [] } });
    await f.swampReply("workflow.history.get", { id: "run-1", status });
    if (status === "running") {
      const command = (f.p.commands as typeof commands)
        .filter((entry) => entry.SwampRequest?.operation === "workflow.cancel")
        .at(-1)!.SwampRequest!;
      assertEquals(JSON.parse(command.payload), { runId: "run-1" });
      await f.swampReply("workflow.cancel", { cancelled: true });
      assertEquals(cancellation.reply().result, { operations: [{ cancelled: true }] });
    } else {
      if (status === "unknown") assert(cancellation.reply().error.includes("state is uncertain"));
      else assertEquals(cancellation.reply().result, { operations: [], alreadyFinished: true });
      assert(
        !(f.p.commands as typeof commands).some(
          (entry) => entry.SwampRequest?.operation === "workflow.cancel",
        ),
      );
    }
  });
}

for (const [diagnostic, expected] of [
  [
    'Module not found "file:///repo/.swamp/pulled-extensions/@swamp/git/models/git.ts"',
    "swamp extension pull @swamp/git --repo-dir '/repo'",
  ],
  ["@hammz/yinz could not be resolved", "swamp doctor extensions --repo-dir '/repo' --json"],
  ["Network unavailable", ""],
]) {
  test(`workflow setup diagnostics preserve the cause: ${diagnostic}`, async () => {
    const f = await cleanupFixture();
    const request = await f.request({
      method: "swamp_read",
      directory: "/repo",
      operation: "workflow.history.get",
      payload: { workflowIdOrName: "run-1" },
    });
    await f.swampReply("workflow.history.get", {
      error: { code: "not_dispatched", message: diagnostic },
    });
    const error = request.reply().error;
    if (expected) assert(error.includes(expected));
    else assert(!error.includes("swamp extension pull") && !error.includes("swamp doctor"));
    assert(error.includes(diagnostic.split('"')[0]));
  });
}

test("failed launch validation names the control checkout and extension diagnostic command", async () => {
  const f = await cleanupFixture();
  const launch = await f.request({
    method: "workflow_run",
    work: "exit",
    key: "launch",
    directory: "/repo",
    exactInputs: true,
  });
  await f.swampReply("workflow.get", { name: "fixture" });
  await f.swampReply("workflow.validate", {
    passed: false,
    errors: ["@hammz/yinz could not be resolved"],
  });
  assert(launch.reply().error.includes("swamp doctor extensions --repo-dir '/repo' --json"));
  assert(!launch.reply().error.includes("swamp extension pull"));
  assert(!JSON.stringify(f.p.commands).includes('"operation":"workflow.run"'));
  f.seed("exit", "launch", { outcome: "failed", error: launch.reply().error, checkout: "/repo" });
  const shown = await f.request({ method: "workflow_show", launch: "exit", directory: "/repo" });
  await f.swampReply("managed.endpoint", {});
  assert(
    shown.reply().result.problem.includes("swamp doctor extensions --repo-dir '/repo' --json"),
  );
});

function closedHelpers(p: Awaited<ReturnType<typeof loadPlugin>>) {
  return p.commands.filter(
    (command) => typeof command === "object" && command != null && "CloseTerminalPane" in command,
  );
}
test("cleanup closes only owned helpers, deduplicates panes and preserves the source on retries", async () => {
  const f = await cleanupFixture();
  f.seed("exit", "view", { view: 2, source: 7 });
  const owner = { ...f.identity, work: "exit", agent: "helper" };
  for (const [key, pane] of [
    ["source", 7],
    ["helper", 9],
    ["duplicate", 9],
  ] as const)
    f.seed("exit", key, { owner, pane, terminationVerified: true });
  for (const field of ["host_id", "session", "incarnation", "work"])
    f.seed("exit", field, { owner: { ...owner, [field]: "foreign" }, pane: 10 });
  f.seed("other", "helper", { owner: { ...owner, work: "other" }, pane: 11 });
  f.seed("exit", "method", {
    request: JSON.stringify({ operation: "model.method.run" }),
    outcome: "pending",
  });
  assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
    ready: false,
  });
  assertEquals(closedHelpers(f.p), []);
  f.seed("exit", "method", {
    request: JSON.stringify({ operation: "model.method.run" }),
    outcome: "completed",
  });
  for (let attempt = 0; attempt < 2; attempt++) {
    const count = f.p.commands.length;
    assertEquals((await f.request({ method: "cleanup_prepare", work: "exit" })).reply().result, {
      ready: true,
    });
    assertEquals(closedHelpers(f.p).length, attempt + 1);
    assertEquals(closedHelpers(f.p).at(-1), { CloseTerminalPane: 9 });
    assert(!JSON.stringify(f.p.commands.slice(count)).includes("ClosePluginPane"));
  }
});
