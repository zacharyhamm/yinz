import { test, session } from "zellij:testing";
test("swamp-workflow renders through the real plugin host", async () => {
  session.start();
  session.loadPlugin("zellij:swamp-workflow");
  await session.waitForText("No run attached");
  session.key("End");
  session.key("Home");
  session.resize(30, 100);
  await session.waitForText("No run attached");
});

import { assertEquals } from "zellij:testing";
import { failedWorkflow } from "./testing-fixtures/workflow.ts";
test(
  "swamp-workflow attaches over CLI pipes and displays authenticated RPC errors",
  { grants: ["RunCommands"] },
  async () => {
    const directory = failedWorkflow();
    session.start({ backgroundPlugins: ["zellij:swamp-bridge"] });
    session.loadPlugin("zellij:swamp-workflow");
    await session.waitForText("No run attached");
    assertEquals(
      JSON.parse(
        session.pipe("show", JSON.stringify({ directory, run: "missing-run" }), {
          plugin: "zellij:swamp-workflow",
        }),
      ),
      { attached: true },
    );
    await session.waitForText("Swamp");
    session.key("Home");
    session.key("End");
    session.resize(30, 100);
    await session.waitForText("Swamp");
  },
);
