import { test, session, assert } from "yinz:testing";
import type { PaneInfo } from "yinz:plugin";

function rightClick(pane: PaneInfo): void {
  session.mouse({
    event_type: "Press",
    right: true,
    left: false,
    middle: false,
    wheel_up: false,
    wheel_down: false,
    shift: false,
    alt: false,
    ctrl: false,
    position: { line: Number(pane.pane_content_y) + 1, column: Number(pane.pane_content_x) + 1 },
  });
}

for (const action of ["Duplicate", "Fork"]) {
  test(
    `context-menu ${action} launches from a hook-reported agent`,
    { grants: ["RunCommands"] },
    async () => {
      session.fixture(
        {},
        {
          bwrap: "exit 1\n",
          claude: "printf 'DUPLICATED AGENT READY\\r\\n'\nwhile IFS= read -r line; do :; done\n",
          codex: "printf 'FORKED AGENT READY\\r\\n'\nwhile IFS= read -r line; do :; done\n",
        },
      );
      session.start({
        columns: 180,
        rows: 36,
        hooks: { context_menu: "yinz:context-menu" },
        backgroundPlugins: [{ path: "yinz:agent-service", configuration: { shared: "true" } }],
      });
      await session.waitForText("yinz-test>");
      session.pipe("ready", "", { plugin: "yinz:context-menu" });
      const source = session.panes.find((pane) => !pane.is_plugin && pane.is_focused);
      assert(source);
      session.report({ Terminal: source.id }, "claude", {
        hook_event_name: "SessionStart",
        session_id: "context-menu-source",
        source: "startup",
      });
      await session.waitFor("source has an agent record", () =>
        session.panes.some((pane) => pane.id === source.id && pane.agent?.kind === "claude"),
      );
      rightClick(source);
      await session.waitForText("Duplicate agent");
      await session.waitForText("Fork agent");
      session.key("Down");
      if (action === "Fork") session.key("Down");
      session.key("Enter");
      if (action === "Fork") {
        await session.waitForText("forking claude");
        session.key("Tab");
        session.key("Enter");
      }
      await session.waitForText(
        action === "Fork" ? "FORKED AGENT READY" : "DUPLICATED AGENT READY",
      );
      assert(session.panes.some((pane) => pane.id === source.id && !pane.is_plugin));
    },
  );
}

test("context-menu opens through real mouse routing", async () => {
  session.start({ hooks: { context_menu: "yinz:context-menu" } });
  await session.waitForText("yinz-test>");
  session.pipe("ready", "", { plugin: "yinz:context-menu" });
  session.mouse({
    event_type: "Press",
    right: true,
    left: false,
    middle: false,
    wheel_up: false,
    wheel_down: false,
    shift: false,
    alt: false,
    ctrl: false,
    position: { line: 5, column: 10 },
  });
  await session.waitForText("Close");
  assert(!session.screen.includes("Copy"));
  session.key("Down");
  session.key("Enter");
  await session.waitFor(
    "menu splits the terminal",
    () => session.panes.filter((pane) => !pane.is_plugin).length === 2,
  );
  session.key("Esc");
  await session.waitFor("menu dismissed", () => !session.screen.includes("Close"));
});
