// Typed raw commands: queries, named fields, tuples, enums, bytes, and permission errors.
import z, { definePlugin, type PaneId, type TabInfo } from "yinz:plugin";
function required<T>(value: T): NonNullable<T> {
  if (value == null) throw new Error("Missing required plugin state");
  return value;
}

let command = "date";
let last = "press a key";
let tabs: TabInfo[] = [];
let opened: PaneId | null = null;
let panes = 0;
const focused = (): TabInfo | undefined => tabs.find((tab) => tab.active);
const actions: Record<string, () => unknown> = {
  i: () => z.getPluginIds(),
  v: () => z.command("GetYinzVersion"),
  p: () => z.command("GetFocusedPaneInfo"),
  d: () => `${z.command("DumpLayout", "default").length} bytes of kdl`,
  t: () => z.command("NewTab", { name: "playground", cwd: null }),
  r: () =>
    focused()
      ? z.command("RenameTab", [required(focused()).position, "renamed-by-typescript"])
      : "no focused tab",
  n: () => (opened = z.command("OpenTerminal", { path: "/tmp" })),
  c: () =>
    (opened = z.command("OpenCommandPane", [
      { path: "sh", args: ["-c", command + "; sleep 30"] },
      {},
    ])),
  w: () =>
    opened
      ? z.command("WriteToPaneId", [z.encodeUtf8("echo written by typescript\r"), opened])
      : "press c first",
  "+": () => z.command("Resize", "Increase"),
  "-": () => z.command("Resize", "Decrease"),
  // Permission deliberately omitted.
  x: () => z.command("CopyToClipboard", "this will not be copied"),
  q: () => z.command("CloseSelf"),
};
z.on("TabUpdate", (value) => {
  tabs = value;
  return true;
});
z.on("PaneUpdate", (value) => {
  panes = Object.values(value.panes).flat().length;
  return true;
});
z.on("PermissionRequestResult", (result) => {
  last = `permissions: ${result}`;
  return true;
});
z.on("Key", (event) => {
  const key =
    event.bare_key === "Esc"
      ? "q"
      : typeof event.bare_key === "object" && "Char" in event.bare_key
        ? event.bare_key.Char
        : "";
  if (!actions[key]) return false;
  try {
    last = `${key} -> ${JSON.stringify(actions[key]()) ?? "ok"}`;
  } catch (error) {
    last = `${key} failed: ${String(error)}`;
  }
  return true;
});
export default definePlugin({
  defaults: { command: "date" },
  load(config) {
    command = config["command"];
    z.subscribe(["Key", "TabUpdate", "PaneUpdate", "PermissionRequestResult"]);
    z.requestPermission([
      "ReadApplicationState",
      "ChangeApplicationState",
      "OpenTerminalsOrPlugins",
      "RunCommands",
      "WriteToStdin",
    ]);
  },
  render(rows, cols) {
    return [
      "yinz TypeScript playground",
      `${tabs.length} tabs, ${panes} panes, focused: ${focused()?.name ?? "-"}`,
      "",
      last,
      "",
      "queries",
      "i plugin ids  v version  p focused pane  d layout",
      "commands",
      "t new tab  r rename tab  n terminal in /tmp",
      `c run ${command}  w write to pane  +/- resize`,
      "x command without permission",
      `q close (${rows}x${cols})`,
    ].join("\n");
  },
});
