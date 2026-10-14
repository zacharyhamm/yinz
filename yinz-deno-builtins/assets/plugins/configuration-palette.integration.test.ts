import { test, session, assert } from "yinz:testing";

test("configuration palette keeps a draft until its destination is submitted", async () => {
  session.start({
    columns: 160,
    backgroundPlugins: [{ path: "yinz:configuration-palette", configuration: { shared: "true" } }],
  });
  await session.waitForText("yinz-test>");
  session.key("Ctrl Alt p");
  session.text("Configure pane frames");
  await session.waitForText("› Configure pane frames…");
  session.key("Enter");
  await session.waitForText("Choose destination…");
  session.key("Right");
  session.key("Enter");
  session.key("Enter");
  await session.waitForText("Apply to session");
  assert(session.screen.includes("Apply change"));
  session.key("Enter");
  session.key("Enter");
  await session.waitForText("Applied to this session");
});
