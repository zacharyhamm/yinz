import { test, session, assert } from "zellij:testing";
test("context-menu opens through real mouse routing", async () => {
  session.start({ hooks: { context_menu: "zellij:context-menu" } });
  await session.waitForText("zellij-test>");
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
