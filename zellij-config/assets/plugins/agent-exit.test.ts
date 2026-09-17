import { test, loadPlugin, assert, assertEquals } from "zellij:testing";

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
  const captured = plugin.commands.at(-1);
  assert(typeof captured === "object" && captured != null && "RunCommand" in captured);
  const payload = captured.RunCommand;
  assert(Array.isArray(payload));
  const context = payload[3];
  assert(typeof context === "object" && context != null);
  await plugin.event({ RunCommandResult: [exitCode, bytes(stdout), bytes(stderr), context] });
}
async function checkout(
  configuration: Record<string, string> = {},
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
  await reply(plugin, listing());
  return plugin;
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
  assert(!(await plugin.render()).includes("delete worktree"));
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
  assert(!(await plugin.render()).includes("delete worktree"));
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
  assert(!(await plugin.render()).includes("delete worktree"));
});
test("agent-exit shows project configuration errors without inspecting Git", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo/topic", agent_target: "local" },
    replies: {
      ProjectConfigRead: { config: { workflow: null }, error: "malformed project config" },
    },
  });
  assert((await plugin.render()).includes("malformed project config"));
  assert(!JSON.stringify(plugin.commands).includes("RunCommand"));
  assert(!(await plugin.render()).includes("Delete:"));
});
test("agent-exit shows thrown project reads without inspecting Git", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo/topic", agent_target: "local" },
  });
  assert((await plugin.render()).includes("unstubbed host operation ProjectConfigRead"));
  assert(!JSON.stringify(plugin.commands).includes("RunCommand"));
  assert(!(await plugin.render()).includes("Delete:"));
});
test("agent-exit confirms and cancels secondary worktree deletion", async () => {
  const plugin = await checkout();
  assert((await plugin.render()).includes("Delete: delete worktree and branch"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  const confirmation = await plugin.render();
  assert(confirmation.includes("/repo/topic"));
  assert(confirmation.includes("topic"));
  await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
  assert(!(await plugin.render()).includes("Delete again"));
});
test("agent-exit revalidates then removes the worktree and branch", async () => {
  const plugin = await checkout();
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, listing());
  await reply(plugin, "");
  await reply(plugin, "");
  const commands = JSON.stringify(plugin.commands);
  assert(commands.includes('"worktree","remove","--force","--","/repo/topic"'));
  assert(commands.includes(`"update-ref","-d","refs/heads/topic","${head}"`));
  assertEquals(plugin.commands.at(-2), { CloseTerminalPane: 7 });
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit refuses branch deletion when the worktree disappeared externally", async () => {
  const plugin = await checkout();
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, listing(false));
  assert((await plugin.render()).includes("worktree disappeared"));
  assert(!JSON.stringify(plugin.commands).includes('"update-ref"'));
});
test("agent-exit keeps diagnostics visible and retries after partial cleanup", async () => {
  const plugin = await checkout();
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, listing());
  await reply(plugin, "");
  await reply(plugin, "", 1, "branch is locked");
  assert((await plugin.render()).includes("branch is locked"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, "");
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit treats an already absent branch as completed", async () => {
  const plugin = await checkout();
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, listing());
  await reply(plugin, "");
  await reply(plugin, "");
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit refuses a branch whose head changed", async () => {
  const plugin = await checkout();
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, listing());
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
    agent_zellij_executable: "/bin/zellij",
    agent_zellij_config_file: "/config/config.kdl",
    agent_zellij_config_dir: "/config",
  });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assert((await plugin.render(24, 200)).includes("VM review"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  const first = plugin.commands.at(-1);
  assert(typeof first === "object" && first != null && "RunCommand" in first);
  assert(Array.isArray(first.RunCommand));
  assertEquals(first.RunCommand[1], {
    ZELLIJ_CONFIG_FILE: "/config/config.kdl",
    ZELLIJ_CONFIG_DIR: "/config",
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
    agent_zellij_executable: "/bin/zellij",
  });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
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
    agent_zellij_executable: "/bin/zellij",
  });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
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
    agent_zellij_executable: "/bin/zellij",
  });
  const vm = {
    session: "work",
    name: "review",
    vm_id: "/vm/review.socket",
    worktree: "/repo/topic",
    metrics: { generation: "42:100" },
  };
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, JSON.stringify({ vms: [vm], recoveries: [] }));
  await reply(plugin, "");
  await reply(plugin, "", 1, "branch is locked");
  const closes = JSON.stringify(plugin.commands).match(/"firecracker","close"/g)?.length ?? 0;
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
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
    name: "zellij_rpc_reply",
    source: { Plugin: 2 },
    payload: JSON.stringify({
      version: 1,
      call_id: request.call_id,
      result: { outcome: "completed" },
    }),
  });
  assert(JSON.stringify(plugin.commands).includes("workflow_show"));
});
