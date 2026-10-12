import { test, loadPlugin, assert, assertEquals } from "yinz:testing";
import type { PluginHarness } from "yinz:testing";

function requests(plugin: PluginHarness): { id: string; request: string }[] {
  return plugin.commands.flatMap((command) => {
    if (typeof command === "object" && command !== null && "AgentControl" in command)
      return [command.AgentControl as { id: string; request: string }];
    return [];
  });
}
async function answer(plugin: PluginHarness, value: unknown): Promise<void> {
  const request = requests(plugin).at(-1);
  assert(request);
  await plugin.event({
    CustomMessage: [
      "agent-response",
      JSON.stringify({
        id: request.id,
        response: {
          outcome: "ok",
          output: JSON.stringify(value),
          agents: [],
          spawned: null,
          message: null,
          acknowledged: false,
          turn_id: null,
        },
      }),
    ],
  });
}
const overview = {
  status: { state: "watching", pending: 0, blockers: 0 },
  checkpoint: { revision: 3, value: null },
  roster: {},
  receipts: [],
  delivery_blocked: null,
  operation_in_flight: false,
  gap: false,
  recent: { records: [], next_cursor: null, high_water: 0, retention_gap: false },
};
test("Overview polls only while visible and keeps one request in flight", async () => {
  const plugin = await loadPlugin("./mother-overview.ts", {
    replies: { AgentControl: null },
  });
  await plugin.advance(3000);
  assertEquals(requests(plugin).length, 0);
  await plugin.event({ Visible: true });
  assertEquals(requests(plugin).length, 1);
  await plugin.advance(3000);
  assertEquals(requests(plugin).length, 1);
  await answer(plugin, overview);
  assert((await plugin.render(24, 100)).includes("Give Mother an assignment in Conversation"));
  await plugin.advance(1000);
  assertEquals(requests(plugin).length, 2);
  await plugin.event({ Visible: false });
  await answer(plugin, { ...overview, status: { ...overview.status, state: "paused" } });
  await plugin.advance(3000);
  assertEquals(requests(plugin).length, 2);
  assert((await plugin.render(24, 100)).includes("Supervision: paused"));
});

test("Overview recovery uses the reviewed revision and requires a visible confirmation", async () => {
  const plugin = await loadPlugin("./mother-overview.ts", {
    replies: { AgentControl: null },
  });
  await plugin.event({ Visible: true });
  await answer(plugin, {
    ...overview,
    receipts: [
      { submission: "missing", settled: false, response: { outcome: "status_unavailable" } },
    ],
  });
  const key = async (char: string): Promise<void> =>
    plugin.event({ Key: { bare_key: { Char: char }, key_modifiers: [] } });
  await key("r");
  await plugin.render(2, 20);
  await key("y");
  assertEquals(requests(plugin).length, 1);
  assert((await plugin.render(10, 100)).includes("Work may already have occurred"));
  await key("y");
  assertEquals(JSON.parse(requests(plugin).at(-1)?.request ?? "{}"), {
    Mother: { Resolve: { submission_id: "missing", expect_rev: 3 } },
  });
});

test("A successful refresh does not hide a rejected control", async () => {
  const plugin = await loadPlugin("./mother-overview.ts", {
    replies: { AgentControl: null },
  });
  await plugin.event({ Visible: true });
  await answer(plugin, overview);
  await plugin.event({ Key: { bare_key: { Char: "p" }, key_modifiers: [] } });
  const request = requests(plugin).at(-1);
  assert(request);
  await plugin.event({
    CustomMessage: [
      "agent-response",
      JSON.stringify({
        id: request.id,
        response: { outcome: "rejected", output: null, message: "Control rejected" },
      }),
    ],
  });
  await answer(plugin, overview);
  assert((await plugin.render(24, 100)).includes("Control rejected"));
});
