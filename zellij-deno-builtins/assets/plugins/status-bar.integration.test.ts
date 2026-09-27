import { test, session, assert } from "zellij:testing";

test("embedded status bar receives real mode updates and remains unselectable", async () => {
  session.start({ columns: 400 });
  await session.waitForText("zellij-test>");
  session.loadPlugin("zellij:status-bar");
  await session.waitForText("LOCK");
  await session.waitFor("status bar is unselectable", () =>
    session.panes.some((pane) => pane.plugin_url === "zellij:status-bar" && !pane.is_selectable),
  );
  session.action('SwitchToMode "Pane";');
  await session.waitForText("Fullscreen");
  session.action('SwitchToMode "Normal";');
  await session.waitFor(
    "normal mode restored",
    () => session.screen.includes("LOCK") && !session.screen.includes("Fullscreen"),
  );
  assert(session.panes.filter((pane) => !pane.is_plugin).length === 1);
});
