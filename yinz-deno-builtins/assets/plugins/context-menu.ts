// Headless context_menu hook. Configured IDs retain their actions; invented items
// return their context to this plugin on selection.
import yinz, { definePlugin, type ContextMenuItem } from "yinz:plugin";

const MAX_QUOTED = 24;
const MAX_OUTPUT = 64;
let opener: string | undefined;
let pending: string | undefined;

function shorten(text: string): string {
  // oxlint-disable-next-line typescript/no-misused-spread -- This terminal operation counts Unicode code points, matching the existing cursor behavior.
  const characters = [...text.replaceAll(/\s+/gu, " ").trim()];
  return characters.slice(0, MAX_QUOTED).join("") + (characters.length > MAX_QUOTED ? "…" : "");
}

// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function run(command: string[], context: Record<string, string> = {}): void {
  yinz.command("RunCommand", [command, {}, "", context, MAX_OUTPUT]);
}

function openLink(url: string): void {
  if (opener) {
    run([opener, url]);
  } else {
    pending = url;
  }
}

yinz.on("RunCommandResult", ([exitCode, _stdout, _stderr, context]) => {
  const executable = context["probe"];
  if (!executable || exitCode !== 0) return;
  if (executable === "xdg-open" || !opener) opener = executable;
  if (pending !== undefined) {
    const url = pending;
    pending = undefined;
    openLink(url);
  }
});

yinz.on("ContextMenuRequested", (event) => {
  const items: ContextMenuItem[] = [];
  if ("Terminal" in event.pane_id && yinz.command("GetPaneInfo", event.pane_id)?.agent) {
    items.push(
      { label: "Duplicate agent", context: { action: "duplicate_agent" } },
      { label: "Fork agent", context: { action: "fork_agent" } },
      { separator: true },
    );
  }
  if (event.link?.matched) {
    items.push(
      {
        label: `Open ${shorten(event.link.matched) || event.link.matched}`,
        context: { action: "open_link", url: event.link.matched },
      },
      { separator: true },
    );
  }
  for (const item of event.items) {
    if (!item.separator && item.label === "Copy") {
      if (event.selected_text == null) continue;
      const quoted = shorten(event.selected_text);
      items.push({ ...item, label: quoted ? `Copy "${quoted}"` : item.label });
    } else items.push(item);
  }
  const tidy: ContextMenuItem[] = [];
  for (const item of items) {
    if (!item.separator || (tidy.length > 0 && !tidy.at(-1)?.separator)) tidy.push(item);
  }
  if (tidy.at(-1)?.separator) tidy.pop();
  yinz.command("SetContextMenu", { request_id: event.request_id, items: tidy });
});

yinz.on("ContextMenuItemSelected", ({ item_context: context, context: source }) => {
  if (context["action"] === "open_link" && context["url"]) openLink(context["url"]);
  if (context["action"] !== "duplicate_agent" && context["action"] !== "fork_agent") return;
  if (!("Terminal" in source.pane_id) || !yinz.command("GetPaneInfo", source.pane_id)?.agent)
    return;
  yinz.command("RunAction", [
    {
      LaunchPlugin: {
        plugin: {
          RunPlugin: {
            location: { Yinz: "new-agent" },
            configuration: {
              caller_terminal_pane_id: String(source.pane_id.Terminal),
              [context["action"] === "fork_agent" ? "fork_focused" : "duplicate_focused"]: "true",
              overlay: "true",
              modal: "true",
            },
            initial_cwd: null,
          },
        },
        should_float: true,
        should_open_in_place: false,
        close_replaced_pane: false,
        skip_cache: false,
        cwd: null,
        no_focus: false,
        tab_id: null,
      },
    },
    {},
  ]);
});

export default definePlugin({
  load() {
    opener = undefined;
    pending = undefined;
    yinz.subscribe(["ContextMenuRequested", "ContextMenuItemSelected", "RunCommandResult"]);
    run(["xdg-open", "--help"], { probe: "xdg-open" });
    run(["open", "--help"], { probe: "open" });
  },
});
