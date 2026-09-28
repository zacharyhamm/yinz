import { test, session, assert } from "yinz:testing";
test("swamp-bridge initializes its persistent headless delivery store", async () => {
  session.start({ backgroundPlugins: ["yinz:swamp-bridge"] });
  await session.waitForText("yinz-test>");
  await session.waitFor("delivery schema created", () =>
    session.catalog().some((entity) => entity.def.name === "delivery"),
  );
  assert(session.data("yinz:swamp-bridge", "delivery").rows.length === 0);
});

import { assertEquals } from "yinz:testing";
import { failedWorkflow, deliveryValues } from "./testing-fixtures/workflow.ts";
test(
  "swamp-bridge retains correlated failure data across headless reload",
  { grants: ["RunCommands"] },
  async () => {
    const directory = failedWorkflow();
    session.start({ backgroundPlugins: ["yinz:swamp-bridge"] });
    await session.waitFor("delivery schema created", () =>
      session.catalog().some((entity) => entity.def.name === "delivery"),
    );
    session.loadPlugin("yinz:agent-exit", {
      configuration: { agent_cwd: directory, agent_exit_token: "persisted-launch" },
    });
    await session.waitFor("failed delivery", () =>
      deliveryValues().some((value) => value.outcome === "failed"),
    );
    const failed = deliveryValues().find((value) => value.outcome === "failed");
    assertEquals(JSON.parse(failed.request).work, "persisted-launch");
    session.reload("yinz:swamp-bridge");
    session.loadPlugin("yinz:swamp-workflow");
    await session.waitForText("No run attached");
    assertEquals(
      JSON.parse(
        session.pipe("show", JSON.stringify({ directory, launch: "persisted-launch" }), {
          plugin: "yinz:swamp-workflow",
        }),
      ),
      { attached: true },
    );
    await session.waitForText("failed");
    await session.waitForText("Swamp");
    // The launch correlation id is available in the execution details.
    session.resize(40, 160);
    await session.waitForText("1/1 Details");
    session.key("Enter");
    session.key("Home");
    await session.waitForText("persisted-launch");
    assertEquals(
      deliveryValues().find((value) => value.outcome === "failed"),
      failed,
    );
  },
);
