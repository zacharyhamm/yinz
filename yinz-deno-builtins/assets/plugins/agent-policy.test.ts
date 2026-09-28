import { test, loadPlugin, assert, assertEquals } from "yinz:testing";
test("agent-policy logs session hooks without notifying", async () => {
  const p = await loadPlugin("./agent-policy.ts");
  await p.event({
    Hook: { point: "session_start", payload: { session_name: "test", client_id: 1 } },
  });
  assert(p.logs.some((line) => line.includes("session_start")));
  assertEquals(p.commands, [{ Subscribe: ["Hook"] }]);
});
test("agent-policy waiting notification targets the agent pane", async () => {
  const p = await loadPlugin("./agent-policy.ts", { replies: { Notify: null } });
  await p.event({
    Hook: {
      point: "agent_waiting",
      payload: { pane_id: 7, agent: { kind: "codex", detail: "needs an answer" } },
    },
  } as never);
  assert(p.logs.some((line) => line.includes("agent_waiting")));
  assertEquals(p.commands.at(-1), {
    Notify: ["codex in pane 7: needs an answer", { Terminal: 7 }, null],
  });
});
