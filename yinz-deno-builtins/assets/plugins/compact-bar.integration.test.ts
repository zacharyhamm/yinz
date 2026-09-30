import { test, session } from "yinz:testing";

test("embedded compact bar receives tabs and opens an unselectable tooltip", async () => {
  session.start({ columns: 240 });
  await session.waitForText("yinz-test>");
  session.action('SwitchToMode "Tab";');
  session.key("r");
  session.text("typescript-compact");
  session.action('SwitchToMode "Normal";');
  session.loadPlugin("yinz:compact-bar", { configuration: { tooltip: "Ctrl y" } });
  const bars = () => session.panes.filter((pane) => pane.plugin_url === "yinz:compact-bar");
  await session.waitForText("typescript-compact");
  await session.waitFor("compact bar is unselectable", () =>
    bars().some((pane) => !pane.is_selectable),
  );

  session.action('SwitchToMode "Pane";');
  await session.waitFor("floating tooltip is unselectable", () =>
    bars().some((pane) => pane.is_floating && !pane.is_selectable),
  );
  session.action('SwitchToMode "Normal";');
  await session.waitFor("transient tooltip closed", () => bars().length === 1);
});
