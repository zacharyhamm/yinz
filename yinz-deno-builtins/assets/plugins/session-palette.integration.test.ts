import { test, session, assert } from "yinz:testing";

test("session palette forms and acknowledged saving work in a real session", async () => {
  session.start({
    columns: 160,
    backgroundPlugins: [{ path: "yinz:session-palette", configuration: { shared: "true" } }],
  });
  await session.waitForText("yinz-test>");
  session.key("Ctrl Alt p");
  session.text("New session");
  await session.waitForText("› New session…");
  session.key("Enter");
  await session.waitForText("Name (optional):");
  session.text("preserved-name");
  session.key("Enter");
  session.key("Down");
  session.key("Enter");
  await session.waitForText("Configure new session");
  assert(session.screen.includes("preserved-name"));
  assert(session.screen.includes("Layout (type to search)"));
  session.key("Esc");
  await session.waitFor(
    "original form restored",
    () => session.screen.includes("preserved-name") && !session.screen.includes("Directory:"),
  );
  session.key("Ctrl Alt p");
  await session.waitFor("palette closed", () => !session.screen.includes("Name (optional):"));
  session.key("Ctrl Alt p");
  session.text("Save current session");
  await session.waitForText("› Save current session");
  session.key("Enter");
  await session.waitForText("Session saved");
});
