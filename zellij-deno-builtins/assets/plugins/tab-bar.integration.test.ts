import { test, session, assert } from "zellij:testing";

test("embedded tab bar receives tab updates and remains unselectable", async () => {
  session.start({ columns: 240 });
  await session.waitForText("zellij-test>");
  session.loadPlugin("zellij:tab-bar");
  await session.waitFor("tab bar is unselectable", () =>
    session.panes.some((pane) => pane.plugin_url === "zellij:tab-bar" && !pane.is_selectable),
  );
  session.action('SwitchToMode "Tab";');
  session.key("r");
  session.text("typescript-tab");
  session.action('SwitchToMode "Normal";');
  await session.waitForText("typescript-tab");
  session.action("NewTab;");
  await session.waitFor(
    "second tab opened",
    () => session.panes.filter((pane) => !pane.is_plugin).length === 2,
  );
  session.action('SwitchToMode "Tab";');
  session.key("r");
  session.text("second-tab");
  session.action('SwitchToMode "Normal";');
  session.action("GoToPreviousTab;");
  await session.waitFor(
    "original tab restored",
    () => session.screen.includes("typescript-tab") && session.screen.includes("second-tab"),
  );
  assert(session.panes.some((pane) => pane.plugin_url === "zellij:tab-bar"));
});
