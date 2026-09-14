import { test, session, assert } from "zellij:testing";
test("agent-exit visible fallback handles keys, resize and reload", async () => {
  session.start();
  session.loadPlugin("zellij:agent-exit");
  await session.waitForText("restart agent");
  session.resize(30, 100);
  await session.waitForText("restart agent");
  session.reload("zellij:agent-exit");
  await session.waitForText("restart agent");
  session.key("Esc");
  await session.waitFor("fallback pane closes", () => !session.screen.includes("restart agent"));
  assert(!session.screen.includes("restart agent"));
});

import { failedWorkflow, deliveryValues } from "./testing-fixtures/workflow.ts";
test(
  "agent-exit attaches to the real bridge and renders a persisted launch failure",
  { grants: ["RunCommands"] },
  async () => {
    const directory = failedWorkflow();
    session.start({ backgroundPlugins: ["zellij:swamp-bridge"] });
    session.loadPlugin("zellij:agent-exit", {
      configuration: { agent_cwd: directory, agent_exit_token: "fixture-launch" },
    });
    await session.waitForText("failed");
    await session.waitFor("launch error persisted", () =>
      deliveryValues().some(
        (value) => value.outcome === "failed" && String(value.error).includes("Swamp"),
      ),
    );
    await session.waitForText("fixture-launch");
    session.resize(30, 100);
    session.key("Home");
    await session.waitForText("failed");
  },
);
