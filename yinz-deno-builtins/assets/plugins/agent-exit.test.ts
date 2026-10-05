import { test, loadPlugin, assert, assertEquals } from "yinz:testing";

const head = "0123456789012345678901234567890123456789";
const listing = (secondary = true): string =>
  `worktree /repo\0HEAD ${head}\0branch refs/heads/main\0\0${
    secondary ? `worktree /repo/topic\0HEAD ${head}\0branch refs/heads/topic\0\0` : ""
  }`;
function bytes(value: string): number[] {
  return [...value].map((character) => character.codePointAt(0) ?? 0);
}
async function reply(
  plugin: Awaited<ReturnType<typeof loadPlugin>>,
  stdout: string,
  exitCode = 0,
  stderr = "",
): Promise<void> {
  const captured = [...plugin.commands]
    .reverse()
    .find((entry) => typeof entry === "object" && entry != null && "RunCommand" in entry);
  assert(typeof captured === "object" && captured != null && "RunCommand" in captured);
  const payload = captured.RunCommand;
  assert(Array.isArray(payload));
  const context = payload[3];
  assert(typeof context === "object" && context != null);
  await plugin.event({ RunCommandResult: [exitCode, bytes(stdout), bytes(stderr), context] });
}
async function checkout(
  configuration: Record<string, string> = {},
  listingOutput = listing(),
): Promise<Awaited<ReturnType<typeof loadPlugin>>> {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: {
      agent_cwd: "/repo/topic",
      agent_pane_id: "7",
      agent_target: "local",
      ...configuration,
    },
    replies: {
      ProjectConfigRead: { config: { workflow: null }, error: null },
      RunCommand: null,
      RequestPluginPermissions: null,
      CloseSelf: null,
      RerunCommandPane: null,
      CloseTerminalPane: null,
    },
  });
  await reply(plugin, "/repo/topic\n");
  await reply(plugin, listingOutput);
  return plugin;
}

async function prepareDeletion(
  plugin: Awaited<ReturnType<typeof loadPlugin>>,
  listingOutput = listing(),
): Promise<void> {
  const before = plugin.commands.length;
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  if (
    plugin.commands
      .slice(before)
      .some((entry) => typeof entry === "object" && entry != null && "RunCommand" in entry)
  )
    await reply(plugin, listingOutput);
  await plugin.render(24, 200);
}
async function confirmDeletion(
  plugin: Awaited<ReturnType<typeof loadPlugin>>,
  listingOutput = listing(),
): Promise<void> {
  await prepareDeletion(plugin, listingOutput);
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
}

async function cleanCheckout(plugin: Awaited<ReturnType<typeof loadPlugin>>): Promise<void> {
  assert(
    JSON.stringify(plugin.commands.at(-1)).includes(
      '"status","--porcelain","--untracked-files=all"',
    ),
  );
  await reply(plugin, "");
  assert(JSON.stringify(plugin.commands.at(-1)).includes('"rev-list","--max-count=1"'));
  await reply(plugin, "");
}

for (const [key, commands] of [
  ["Enter", ["RerunCommandPane", "CloseSelf"]],
  ["Esc", ["CloseTerminalPane", "CloseSelf"]],
] as const) {
  test(`agent-exit fallback ${key}`, async () => {
    const plugin = await loadPlugin("./agent-exit.ts", {
      configuration: { agent_pane_id: "7" },
      replies: { CloseSelf: null, RerunCommandPane: null, CloseTerminalPane: null },
    });
    assert((await plugin.render()).includes("restart agent"));
    await plugin.event({ Key: { bare_key: key, key_modifiers: [] } });
    assertEquals(
      plugin.commands
        .slice(-commands.length)
        .map((command) =>
          typeof command === "string" ? command : Object.keys(command as object)[0],
        ),
      [...commands],
    );
  });
}
test("agent-exit offers no deletion without a secondary worktree", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_pane_id: "7" },
  });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assertEquals(
    plugin.commands.filter((command) => command === "CloseSelf"),
    [],
  );
  assert(!(await plugin.render()).includes("Delete worktree"));
});
test("agent-exit offers no deletion for a primary checkout", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo", agent_target: "local" },
    replies: {
      ProjectConfigRead: { config: { workflow: null }, error: null },
      RunCommand: null,
    },
  });
  await reply(plugin, "/repo\n");
  await reply(plugin, listing(false));
  assert(!(await plugin.render()).includes("Delete worktree"));
});
test("agent-exit offers no deletion outside a repository", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo", agent_target: "local" },
    replies: {
      ProjectConfigRead: { config: { workflow: null }, error: null },
      RunCommand: null,
    },
  });
  await reply(plugin, "", 128, "not a repository");
  assert(!(await plugin.render()).includes("Delete worktree"));
});
test("agent-exit shows project configuration errors while inspecting Git", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo/topic", agent_target: "local" },
    replies: {
      ProjectConfigRead: { config: { workflow: null }, error: "malformed project config" },
      RunCommand: null,
    },
  });
  await reply(plugin, "/repo/topic\n");
  await reply(plugin, listing());
  assert((await plugin.render()).includes("malformed project config"));
  assert(JSON.stringify(plugin.commands).includes("RunCommand"));
  assert((await plugin.render()).includes("Delete worktree"));
});
test("agent-exit shows thrown project reads while inspecting Git", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo/topic", agent_target: "local" },
    replies: { RunCommand: null },
  });
  await reply(plugin, "/repo/topic\n");
  await reply(plugin, listing());
  assert((await plugin.render()).includes("unstubbed host operation ProjectConfigRead"));
  assert(JSON.stringify(plugin.commands).includes("RunCommand"));
  assert((await plugin.render()).includes("Delete worktree"));
});
test("agent-exit confirms and cancels secondary worktree deletion", async () => {
  const plugin = await checkout();
  assert((await plugin.render()).includes("Delete: Delete worktree & branch and close pane"));
  await prepareDeletion(plugin);
  const confirmation = await plugin.render();
  assert(confirmation.includes("/repo/topic"));
  assert(confirmation.includes("topic"));
  await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
  assert(!(await plugin.render()).includes("Delete again"));
});
test("agent-exit revalidates then removes the worktree and branch", async () => {
  const plugin = await checkout();
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  await reply(plugin, "");
  await reply(plugin, "");
  const commands = JSON.stringify(plugin.commands);
  assert(commands.includes('"worktree","remove","--","/repo/topic"'));
  assert(commands.includes(`"update-ref","-d","refs/heads/topic","${head}"`));
  assertEquals(plugin.commands.at(-2), { CloseTerminalPane: 7 });
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
for (const target of ["local", "firecracker"]) {
  for (const changes of [" M tracked\n", "?? untracked\n", "M  staged\n"]) {
    test(`agent-exit preserves ${target} dirty worktree and branch: ${changes.trim()}`, async () => {
      const plugin = await checkout({
        agent_target: target,
        agent_firecracker_vm: "review",
        agent_session: "work",
        agent_yinz_executable: "/bin/yinz",
      });
      await confirmDeletion(plugin);
      await reply(plugin, listing());
      await reply(plugin, changes);
      assert((await plugin.render(24, 200)).includes("uncommitted changes"));
      const commands = JSON.stringify(plugin.commands);
      assert(!commands.includes('"worktree","remove"'));
      assert(!commands.includes('"update-ref"'));
      assert(!commands.includes('"firecracker","close"'));
      assert(!plugin.commands.includes("CloseSelf"));
    });
  }
  test(`agent-exit preserves ${target} commits not merged into the configured base`, async () => {
    const plugin = await checkout({
      agent_target: target,
      agent_firecracker_vm: "review",
      agent_session: "work",
      agent_yinz_executable: "/bin/yinz",
    });
    plugin.stub("ProjectConfigRead", {
      config: { workflow: null, default_branch: "trunk" },
      error: null,
    });
    await confirmDeletion(plugin);
    await reply(plugin, listing());
    await reply(plugin, "");
    assert(JSON.stringify(plugin.commands.at(-1)).includes(`refs/heads/trunk..${head}`));
    await reply(plugin, head + "\n");
    assert((await plugin.render(24, 200)).includes("not merged into trunk"));
    const commands = JSON.stringify(plugin.commands);
    assert(!commands.includes('"worktree","remove"'));
    assert(!commands.includes('"update-ref"'));
    assert(!commands.includes('"firecracker","close"'));
    assert(!plugin.commands.includes("CloseSelf"));
  });
}
test("agent-exit refuses branch deletion when the worktree disappeared externally", async () => {
  const plugin = await checkout();
  await confirmDeletion(plugin);
  await reply(plugin, listing(false));
  assert((await plugin.render()).includes("worktree disappeared"));
  assert(!JSON.stringify(plugin.commands).includes('"update-ref"'));
});
test("agent-exit keeps diagnostics visible and retries after partial cleanup", async () => {
  const plugin = await checkout();
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  await reply(plugin, "");
  await reply(plugin, "", 1, "branch is locked");
  const failure = await plugin.render();
  assert(failure.includes("branch is locked"));
  assert(failure.includes("Worktree removed; branch cleanup remains."));
  assert(!failure.includes("restart agent"));
  const count = plugin.commands.length;
  await plugin.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
  await prepareDeletion(plugin);
  await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
  assert((await plugin.render()).includes("branch is locked"));
  await confirmDeletion(plugin);
  await reply(plugin, "");
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit treats an already absent branch as completed", async () => {
  const plugin = await checkout();
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  await reply(plugin, "");
  await reply(plugin, "");
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit refuses a branch whose head changed", async () => {
  const plugin = await checkout();
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  await reply(plugin, "");
  await reply(plugin, "", 128, "cannot lock ref: is at another head but expected original");
  assert((await plugin.render()).includes("cannot lock ref"));
  assert(!plugin.commands.includes("CloseSelf"));
});
test("agent-exit closes a guarded Firecracker VM before Git deletion", async () => {
  const plugin = await checkout({
    agent_target: "firecracker",
    agent_firecracker_worktree: "/repo/topic",
    agent_firecracker_vm: "review",
    agent_session: "work",
    agent_yinz_executable: "/bin/yinz",
    agent_yinz_config_file: "/config/config.kdl",
    agent_yinz_config_dir: "/config",
  });
  await prepareDeletion(plugin);
  assert((await plugin.render(24, 200)).includes("VM: review"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  const first = plugin.commands.at(-1);
  assert(typeof first === "object" && first != null && "RunCommand" in first);
  assert(Array.isArray(first.RunCommand));
  assertEquals(first.RunCommand[1], {
    YINZ_CONFIG_FILE: "/config/config.kdl",
    YINZ_CONFIG_DIR: "/config",
  });
  const vm = {
    session: "work",
    name: "review",
    vm_id: "/vm/review.socket",
    worktree: "/repo/topic",
    metrics: { generation: "42:100" },
  };
  await reply(plugin, JSON.stringify({ vms: [vm], recoveries: [] }));
  await reply(plugin, "");
  await reply(plugin, "");
  const commands = JSON.stringify(plugin.commands);
  assert(commands.includes('"close","review","--session","work","--expected-identity"'));
  assert(commands.includes('"--expected-worktree","/repo/topic","--expected-head"'));
  assert(!commands.includes('"worktree","remove"'));
});
test("agent-exit refuses Git deletion while another VM lease remains", async () => {
  const plugin = await checkout({
    agent_target: "firecracker",
    agent_firecracker_worktree: "/repo/topic",
    agent_firecracker_vm: "review",
    agent_session: "work",
    agent_yinz_executable: "/bin/yinz",
  });
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  const vm = {
    session: "work",
    name: "review",
    vm_id: "/vm/review.socket",
    worktree: "/repo/topic",
    metrics: { generation: "42:100" },
  };
  await reply(plugin, JSON.stringify({ vms: [vm], recoveries: [] }));
  await reply(plugin, "", 1, "Worktree is still leased by other");
  assert((await plugin.render(24, 200)).includes("still leased by other"));
  assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
});
test("agent-exit refuses a named Firecracker VM that moved to another worktree", async () => {
  const plugin = await checkout({
    agent_target: "firecracker",
    agent_firecracker_worktree: "/repo/topic",
    agent_firecracker_vm: "review",
    agent_session: "work",
    agent_yinz_executable: "/bin/yinz",
  });
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  await reply(
    plugin,
    JSON.stringify({
      vms: [
        {
          session: "work",
          name: "review",
          vm_id: "/vm/review.socket",
          worktree: "/repo/other",
          metrics: { generation: "42:100" },
        },
      ],
      recoveries: [],
    }),
  );
  assert((await plugin.render()).includes("VM identity changed"));
  assert(!JSON.stringify(plugin.commands).includes('"firecracker","close"'));
});
test("agent-exit retries only branch cleanup after guarded Firecracker removal", async () => {
  const plugin = await checkout({
    agent_target: "firecracker",
    agent_firecracker_worktree: "/repo/topic",
    agent_firecracker_vm: "review",
    agent_session: "work",
    agent_yinz_executable: "/bin/yinz",
  });
  const vm = {
    session: "work",
    name: "review",
    vm_id: "/vm/review.socket",
    worktree: "/repo/topic",
    metrics: { generation: "42:100" },
  };
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  await reply(plugin, JSON.stringify({ vms: [vm], recoveries: [] }));
  await reply(plugin, "");
  await reply(plugin, "", 1, "branch is locked");
  const closes = JSON.stringify(plugin.commands).match(/"firecracker","close"/g)?.length ?? 0;
  await confirmDeletion(plugin);
  await reply(plugin, "");
  assertEquals(
    JSON.stringify(plugin.commands).match(/"firecracker","close"/g)?.length ?? 0,
    closes,
  );
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit leaves configured workflow behavior unchanged", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/fixture", agent_exit_token: "launch-1", agent_pane_id: "7" },
    replies: {
      GetPluginIds: { plugin_id: 1 },
      GetPaneInfo: { is_suppressed: false },
      ProjectConfigRead: { config: { workflow: "fixture" } },
      SetSelfMouseSelectionSupport: null,
      SubscribePaneOutput: null,
      MessageToPlugin: null,
      SetPaneSubtitle: null,
    },
  });
  assert((await plugin.render()).includes("Starting workflow"));
  const messages = plugin.commands as { MessageToPlugin?: { message_payload: string } }[];
  const request = JSON.parse(
    messages.filter((command) => command.MessageToPlugin).at(-1)!.MessageToPlugin!.message_payload,
  );
  assertEquals(request.payload.method, "workflow_run");
  assertEquals(request.payload.work, "launch-1");
  await plugin.pipe({
    name: "yinz_rpc_reply",
    source: { Plugin: 2 },
    payload: JSON.stringify({
      version: 1,
      call_id: request.call_id,
      result: { outcome: "completed" },
    }),
  });
  assert(JSON.stringify(plugin.commands).includes("workflow_show"));
});

function rpcRequest(plugin: Awaited<ReturnType<typeof loadPlugin>>, method: string) {
  const messages = plugin.commands as { MessageToPlugin?: { message_payload: string } }[];
  return messages
    .filter((entry) => entry.MessageToPlugin)
    .map((entry) => JSON.parse(entry.MessageToPlugin!.message_payload))
    .filter((entry) => entry.payload?.method === method)
    .at(-1);
}
async function rpcReply(
  plugin: Awaited<ReturnType<typeof loadPlugin>>,
  method: string,
  result: unknown,
  error?: string,
) {
  const request = rpcRequest(plugin, method);
  assert(request);
  await plugin.pipe({
    name: "yinz_rpc_reply",
    source: { Plugin: 2 },
    payload: JSON.stringify({
      version: 1,
      call_id: request.call_id,
      ...(error ? { error } : { result }),
    }),
  });
}
async function configuredCheckout(
  projectWorkflow: string | null = "fixture",
): Promise<Awaited<ReturnType<typeof loadPlugin>>> {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: {
      agent_cwd: "/repo/topic",
      agent_target: "local",
      agent_pane_id: "7",
      agent_exit_token: "exit-1",
    },
    replies: {
      GetPluginIds: { plugin_id: 1 },
      GetPaneInfo: { is_suppressed: false },
      ProjectConfigRead: { config: { workflow: projectWorkflow } },
      RunCommand: null,
      MessageToPlugin: null,
      SetSelfMouseSelectionSupport: null,
      SubscribePaneOutput: null,
      SetPaneSubtitle: null,
      CloseTerminalPane: null,
      CloseSelf: null,
    },
  });
  await reply(plugin, "/repo/topic\n");
  await reply(plugin, listing());
  return plugin;
}
test("agent-exit preserves normal Delete and Esc editing keys in answers", async () => {
  const plugin = await configuredCheckout();
  await rpcReply(plugin, "workflow_run", { outcome: "completed" });
  await rpcReply(plugin, "workflow_show", {
    launch: "exit-1",
    run: "run-1",
    root: "/repo",
    history: { status: "suspended" },
    questions: {
      results: [
        {
          model: "question",
          version: 1,
          attributes: {
            run: "run-1",
            question: "q",
            state: "pending",
            kind: "text",
            prompt: "What next?",
          },
        },
      ],
    },
  });
  await plugin.render();
  await plugin.event({ PastedText: "abc" });
  const count = plugin.commands.length;
  await plugin.event({ Key: { bare_key: "Home", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
  assert(!(await plugin.render()).includes("Delete again"));
  await plugin.event({ Key: { bare_key: "Tab", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
  assertEquals(rpcRequest(plugin, "workflow_answer").payload.inputs.answer, "bc");
});
test("agent-exit returns to the workflow when checkout inspection fails before cleanup", async () => {
  const plugin = await configuredCheckout();
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, "", 1, "checkout inspection failed");
  const failure = await plugin.render();
  assert(failure.includes("checkout inspection failed"));
  assert(failure.includes("Enter: back to workflow"));
  assert(!failure.includes("Enter: restart agent"));
  assert(!rpcRequest(plugin, "cleanup_prepare"));
  await plugin.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
  assert((await plugin.render()).includes("Starting workflow"));
});
for (const [action, notice] of [
  ["restart", "Restarting agent..."],
  ["fork", "Opening agent picker..."],
]) {
  test(`agent-exit acknowledges ${action} before the bridge responds`, async () => {
    const plugin = await configuredCheckout();
    if (action === "fork") await plugin.event({ Key: { bare_key: "Tab", key_modifiers: [] } });
    await plugin.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
    assertEquals(rpcRequest(plugin, "pane_action").payload.action, action);
    assert((await plugin.render()).includes(notice));
    const count = plugin.commands.length;
    await plugin.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
    assertEquals(plugin.commands.length, count);
  });
}
async function assertHiddenControls(plugin: Awaited<ReturnType<typeof loadPlugin>>) {
  const count = plugin.commands.length;
  // The workflow was rendered at 24x80: its input and buttons occupy these rows.
  for (const row of [21, 22]) {
    for (let column = 0; column < 80; column++) {
      await plugin.event({ Mouse: { LeftClick: [row, column] } });
      await plugin.event({ Mouse: { Release: [row, column] } });
    }
  }
  await plugin.event({ Key: { bare_key: "Tab", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
  await plugin.event({ PastedText: "hidden answer" });
  assertEquals(plugin.commands.length, count);
}
for (const state of ["absent", "starting", "running", "question", "completed", "failed"]) {
  test(`agent-exit cleanup is available while workflow is ${state}`, async () => {
    const plugin = await configuredCheckout(state === "absent" ? null : "fixture");
    if (state !== "absent" && state !== "starting") {
      await rpcReply(plugin, "workflow_run", { outcome: "completed" });
      await rpcReply(plugin, "workflow_show", {
        launch: "exit-1",
        run: "run-1",
        root: "/repo",
        active: state === "running" || state === "question",
        history: {
          id: "run-1",
          workflowName: "fixture",
          status: state === "question" ? "running" : state,
        },
        questions: {
          results:
            state === "question"
              ? [
                  {
                    model: "question",
                    version: 1,
                    attributes: {
                      run: "run-1",
                      question: "q",
                      state: "pending",
                      kind: "text",
                      prompt: "What next?",
                    },
                  },
                ]
              : [],
        },
      });
    }
    if (state === "question") await plugin.event({ Key: { bare_key: "Tab", key_modifiers: [] } });
    assert((await plugin.render()).includes("Delete worktree & branch and close pane"));
    await prepareDeletion(plugin);
    assert((await plugin.render()).includes("This cancels the workflow"));
    assert(!rpcRequest(plugin, "cleanup_prepare"));
    if (state !== "absent") await assertHiddenControls(plugin);
    await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
    assert(!(await plugin.render()).includes("Delete again"));
    await confirmDeletion(plugin);
    await plugin.render();
    if (state !== "absent") await assertHiddenControls(plugin);
    const count = plugin.commands.length;
    await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
    await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
    assertEquals(plugin.commands.length, count);
    assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
    await rpcReply(plugin, "cleanup_prepare", null, "termination uncertain");
    assert((await plugin.render()).includes("termination uncertain"));
    if (state !== "absent") await assertHiddenControls(plugin);
    if (state !== "absent" && state !== "starting") {
      const before = rpcRequest(plugin, "workflow_show")?.call_id;
      await plugin.event({ CommandPaneExited: [9, 0, { swamp_work: "exit-1" }] });
      assert(rpcRequest(plugin, "workflow_show")?.call_id !== before);
    }
    assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
    await confirmDeletion(plugin);
    await rpcReply(plugin, "cleanup_prepare", { ready: true });
    await reply(plugin, listing());
    await cleanCheckout(plugin);
    await reply(plugin, "");
    await reply(plugin, "");
    assertEquals(plugin.commands.at(-1), "CloseSelf");
  });
}

test("agent-exit removes a detached secondary checkout without deleting a branch", async () => {
  const detached = listing().replace("branch refs/heads/topic", "detached");
  const plugin = await checkout({}, detached);
  await prepareDeletion(plugin, detached);
  assert((await plugin.render()).includes("detached HEAD"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, detached);
  await cleanCheckout(plugin);
  await reply(plugin, "");
  assertEquals(plugin.commands.at(-1), "CloseSelf");
  assert(!JSON.stringify(plugin.commands).includes("update-ref"));
});
for (const field of ["head", "branch"]) {
  test(`agent-exit refuses cleanup when the confirmed ${field} changes`, async () => {
    const plugin = await checkout();
    await confirmDeletion(plugin);
    await reply(
      plugin,
      field === "head"
        ? listing().replaceAll(head, "1".repeat(40))
        : listing().replace("refs/heads/topic", "refs/heads/other"),
    );
    assert((await plugin.render()).includes("identity changed"));
    assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
  });
}
test("agent-exit refreshes a changed checkout before confirming its exact revision", async () => {
  const plugin = await checkout();
  const next = "1".repeat(40);
  const changed = listing()
    .replaceAll(head, next)
    .replace("refs/heads/topic", "refs/heads/rebased");
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assert((await plugin.render()).includes("Checking current checkout"));
  const count = plugin.commands.length;
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
  await reply(plugin, changed);
  const confirmation = await plugin.render();
  assert(confirmation.includes(next));
  assert(confirmation.includes("Branch: rebased"));
  assert(confirmation.includes("commits merged into the base branch"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, changed);
  await cleanCheckout(plugin);
  await reply(plugin, "");
  await reply(plugin, "");
  assert(JSON.stringify(plugin.commands).includes(`"refs/heads/rebased","${next}"`));
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit requires the complete confirmation to be visible before deleting", async () => {
  const plugin = await checkout();
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, listing());
  let count = plugin.commands.length;
  // A confirmation that has not rendered yet cannot be accepted by a repeated key.
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
  for (const [rows, cols] of [
    [1, 1],
    [2, 8],
    [8, 24],
  ]) {
    await plugin.render(rows, cols);
    await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
    assertEquals(plugin.commands.length, count);
  }
  assert((await plugin.render(8, 24)).includes("Deletion disabled"));
  assert((await plugin.render(8, 24)).includes("Esc: cancel"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
  await plugin.render(24, 80);
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assert(plugin.commands.length > count);
  count = plugin.commands.length;
  await plugin.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
});
test("agent-exit never inspects an SSH pane's path locally", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo/topic", agent_target: "ssh" },
  });
  assert(!(await plugin.render()).includes("Delete worktree"));
  assert(!JSON.stringify(plugin.commands).includes("RunCommand"));
});

for (const secondary of [false, true]) {
  test(`agent-exit Esc closes configured workflow panes in a ${secondary ? "secondary" : "primary"} checkout`, async () => {
    const directory = secondary ? "/repo/topic" : "/repo";
    const plugin = await loadPlugin("./agent-exit.ts", {
      configuration: {
        agent_cwd: directory,
        agent_target: "local",
        agent_pane_id: "7",
        agent_exit_token: "exit-1",
      },
      replies: {
        GetPluginIds: { plugin_id: 1 },
        GetPaneInfo: { is_suppressed: false },
        ProjectConfigRead: { config: { workflow: "fixture" } },
        RunCommand: null,
        MessageToPlugin: null,
        SetSelfMouseSelectionSupport: null,
        SubscribePaneOutput: null,
        SetPaneSubtitle: null,
        CloseTerminalPane: null,
        CloseSelf: null,
      },
    });
    await reply(plugin, directory + "\n");
    await reply(plugin, listing(secondary));
    assert((await plugin.render(24, 200)).includes("Esc: close pane"));
    if (secondary) {
      await prepareDeletion(plugin);
      await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
      assert(!plugin.commands.includes("CloseSelf"));
    }
    const count = plugin.commands.length;
    await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
    assertEquals(plugin.commands.slice(count), [{ CloseTerminalPane: 7 }, "CloseSelf"]);
    assert(!rpcRequest(plugin, "cleanup_prepare"));
    assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
    assert(!JSON.stringify(plugin.commands).includes('"update-ref"'));
  });
}
