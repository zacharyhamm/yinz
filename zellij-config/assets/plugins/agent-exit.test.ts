import { test, loadPlugin, assert, assertEquals } from "zellij:testing";
for (const [key, command] of [
  ["Enter", "RerunCommandPane"],
  ["Delete", "CloseTerminalPane"],
  ["Esc", null],
] as const) {
  test(`agent-exit fallback ${key}`, async () => {
    const p = await loadPlugin("./agent-exit.ts", {
      configuration: { agent_pane_id: "7" },
      replies: { CloseSelf: null, RerunCommandPane: null, CloseTerminalPane: null },
    });
    assert((await p.render()).includes("restart agent"));
    await p.event({ Key: { bare_key: key, key_modifiers: [] } });
    assertEquals(p.commands.at(-1), "CloseSelf");
    if (command) assertEquals(p.commands.at(-2), { [command]: 7 });
  });
}
test("agent-exit shows project setup errors", async () => {
  const p = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/work" },
    replies: { ProjectConfigRead: { error: "No exit workflow configured." } },
  });
  assert((await p.render()).includes("project-setup"));
});
test("agent-exit attaches its configured workflow through correlated RPC", async () => {
  const p = await loadPlugin("./agent-exit.ts", {
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
  assert((await p.render()).includes("Starting workflow"));
  const messages = p.commands as { MessageToPlugin?: { message_payload: string } }[];
  const request = JSON.parse(
    messages.filter((c) => c.MessageToPlugin).at(-1)!.MessageToPlugin!.message_payload,
  );
  assertEquals(request.payload.method, "workflow_run");
  assertEquals(request.payload.work, "launch-1");
  await p.pipe({
    name: "zellij_rpc_reply",
    source: { Plugin: 2 },
    payload: JSON.stringify({
      version: 1,
      call_id: request.call_id,
      result: { outcome: "completed" },
    }),
  });
  const after = p.commands as { MessageToPlugin?: { message_payload: string } }[];
  assert(JSON.stringify(after).includes("workflow_show"));
});
