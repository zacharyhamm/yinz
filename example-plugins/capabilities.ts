// Live state, subscriptions, timers and raw commands in one pane.
import z, { definePlugin, type TabInfo, type TimerHandle } from "yinz:plugin";
let running = true,
  selectable = true,
  tick = 1000,
  ticks = 0,
  title = "TypeScript capabilities";
let lastKey = "-";
let mode = "?";
let panes = 0;
let timer: TimerHandle;
let tabs: TabInfo[] = [];
const log: string[] = [];
function note(line: string): void {
  log.unshift(line);
  log.splice(5);
}
function start(): void {
  // oxlint-disable-next-line unicorn/no-array-callback-reference -- SDK every takes a numeric interval followed by a callback; it is not Array.every.
  timer = z.every(tick, () => {
    ticks += 1;
    return true;
  });
}
z.on("PermissionRequestResult", (result) => {
  note(`permission result: ${result}`);
  return true;
});
export default definePlugin({
  defaults: { title: "TypeScript capabilities", tick: "1" },
  load(config) {
    title = config["title"];
    tick = (Number(config["tick"]) || 1) * 1000;
    z.requestPermission([
      "ReadApplicationState",
      "ChangeApplicationState",
      "OpenTerminalsOrPlugins",
    ]);
    z.subscribe(["Key", "ModeUpdate", "TabUpdate", "PaneUpdate", "PermissionRequestResult"]);
    start();
    note(`loaded with title=${title} tick=${tick / 1000}`);
  },
  update(event) {
    if (typeof event !== "object") return false;
    if ("ModeUpdate" in event) mode = event.ModeUpdate.mode;
    else if ("TabUpdate" in event) tabs = event.TabUpdate;
    else if ("PaneUpdate" in event) panes = Object.values(event.PaneUpdate.panes).flat().length;
    else if ("Key" in event) {
      const key = event.Key.bare_key;
      lastKey = JSON.stringify(key) + " " + event.Key.key_modifiers.join("+");
      if (key === "Esc") z.command("CloseSelf");
      else if (typeof key === "object" && "Char" in key) {
        if (key.Char === "o") {
          z.command("OpenTerminal", { path: "." });
          note("opened a terminal in .");
        } else if (key.Char === "s") {
          selectable = !selectable;
          z.command("SetSelectable", selectable);
          note(`selectable = ${selectable}`);
        } else if (key.Char === "t") {
          running = !running;
          if (running) start();
          else timer.cancel();
          note(`timer ${running ? "resumed" : "paused"}`);
        } else if (key.Char === "u") {
          z.unsubscribe(["PaneUpdate"]);
          note("unsubscribed from PaneUpdate");
        }
      }
    } else return false;
    return true;
  },
  render(rows, cols) {
    return [
      `${title} (${rows}x${cols})`,
      "",
      `uptime: ${ticks}s ${running ? "" : "(paused)"}`,
      `mode: ${mode}`,
      `last key: ${lastKey}`,
      `tabs: ${tabs.map((tab) => (tab.active ? `[${tab.name}]` : tab.name)).join(", ")}`,
      `panes: ${panes}`,
      `selectable: ${selectable}`,
      "",
      "o open terminal  s selectable  t timer  u unsubscribe  Esc close",
      "",
      ...log,
    ]
      .slice(0, rows)
      .join("\n");
  },
});
