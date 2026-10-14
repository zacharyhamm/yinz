import { test, session, assert } from "yinz:testing";

test("configuration palette submits a scalar value and destination together", async () => {
  session.start({
    columns: 160,
    backgroundPlugins: [{ path: "yinz:configuration-palette", configuration: { shared: "true" } }],
  });
  await session.waitForText("yinz-test>");
  session.key("Ctrl Alt p");
  session.text("Configuration");
  await session.waitForText("› Configuration…");
  session.key("Enter");
  await session.waitForText("simplified ui…");
  session.text("pane frames");
  await session.waitForText("› pane frames…");
  session.key("Enter");
  await session.waitForText("Apply change");
  assert(session.screen.includes("Apply to session"));
  session.key("Right");
  session.key("Enter");
  session.key("Enter");
  session.key("Enter");
  await session.waitForText("Applied to this session");
});
