import z, {
  definePlugin,
  type Event,
  type FloatingPaneCoordinates,
  type InputMode,
  type Keybinds,
  type Mouse,
  type PaneManifest,
  type PipeMessage,
} from "yinz:plugin";
import { defaultMode } from "./status-bar/default-mode.ts";
import { equal, type ModeInfo, type ReadonlyWire } from "./status-bar/keys.ts";
import { background, render, type Bar } from "./compact-bar/render.ts";
import { dimensions, renderTooltip } from "./compact-bar/tooltip.ts";
import type { Part, Range } from "./tab-bar/render.ts";

const state: Bar = {
  hostname: "",
  modeInfo: defaultMode(),
  tabs: [],
  toggleKey: null,
  tooltipActive: false,
};
let activeTab = 0,
  displayRows = 0,
  ownClientId = 0,
  ownPluginId = 0;
let ownTab: number | null = null;
let config: Record<string, string> = {};
let firstRun = false,
  isTooltip = false,
  persist = false;
let clipboardFailed = false,
  copied: "Command" | "Primary" | "System" | null = null;
let cachedKeybinds: ReadonlyWire<Keybinds> = [];
let breadcrumb: Range = null,
  parts: Part[] = [];
const restricted = (mode: InputMode): boolean =>
  ["Locked", "EnterSearch", "RenameTab", "RenamePane", "Prompt", "Tmux", "Select"].includes(mode);

function coordinates(): FloatingPaneCoordinates {
  const [rows, cols] = dimensions(state.modeInfo);
  return {
    x: { Fixed: 2 },
    y: { Fixed: Math.max(0, displayRows - rows - 4) },
    width: { Fixed: cols + 4 },
    height: { Fixed: rows + 2 },
    pinned: true,
    borderless: false,
  };
}

function launch(persisted: boolean): void {
  z.command("MessageToPlugin", {
    plugin_url: "yinz:OWN_URL",
    destination_plugin_id: null,
    plugin_config: { ...config, is_tooltip: "true" },
    message_name: persisted ? "toggle_persisted_tooltip" : "launch_tooltip_if_not_launched",
    message_payload: null,
    message_args: persisted ? { persist: "" } : {},
    new_plugin_args: {
      should_float: null,
      pane_id_to_replace: null,
      pane_title: state.modeInfo.mode,
      cwd: null,
      skip_cache: false,
      should_focus: null,
    },
    floating_pane_coordinates: coordinates(),
  });
}

function modeUpdate(info: ModeInfo): boolean {
  let mode = info;
  if (mode.keybinds.length === 0 && cachedKeybinds.length > 0)
    mode = { ...mode, keybinds: cachedKeybinds };
  else if (mode.keybinds.length > 0) cachedKeybinds = mode.keybinds;
  const changed = !equal(state.modeInfo, mode),
    previous = state.modeInfo.mode;
  const base = mode.base_mode ?? "Normal";
  state.modeInfo = mode;
  if (isTooltip) {
    if (!persist && (mode.mode === base || restricted(mode.mode))) z.command("CloseSelf");
    else if (mode.mode !== previous || persist) {
      z.command("ChangeFloatingPanesCoordinates", [[{ Plugin: ownPluginId }, coordinates()]]);
      z.command("RenamePluginPane", [ownPluginId, mode.mode]);
    }
  } else if (state.toggleKey !== null && mode.mode !== base && !restricted(mode.mode))
    launch(false);
  return changed;
}

function paneUpdate(manifest: ReadonlyWire<PaneManifest>): boolean {
  if (state.toggleKey === null) return false;
  const previous = state.tooltipActive;
  const pluginId = ownPluginId;
  state.tooltipActive = Object.values(manifest.panes).some((panes) =>
    panes.some(
      (pane) => pane.plugin_url === "yinz:compact-bar" && pane.pane_x !== pane.pane_content_x,
    ),
  );
  ownTab = null;
  for (const [tab, panes] of Object.entries(manifest.panes).sort(
    ([a], [b]) => Number(a) - Number(b),
  )) {
    if (panes.some((pane) => pane.is_plugin && pane.id === pluginId)) {
      ownTab = Number(tab);
      break;
    }
  }
  return previous !== state.tooltipActive;
}

function tabAt(col: number): number | null {
  let start = 0;
  for (const part of parts) {
    if (col >= start && col < start + part.len) return part.tab === undefined ? null : part.tab + 1;
    start += part.len;
  }
  return null;
}

function mouse(event: ReadonlyWire<Mouse>): void {
  if (isTooltip) return;
  if ("LeftClick" in event) {
    const col = Number(event.LeftClick[1]);
    if (breadcrumb !== null && col >= breadcrumb[0] && col < breadcrumb[1])
      z.command("FocusHostSession");
    else {
      const tab = tabAt(col);
      if (tab !== null && tab !== activeTab && tab <= 4294967295) z.command("SwitchTabTo", tab);
    }
  } else if ("ScrollUp" in event) {
    const next = Math.min(activeTab + 1, state.tabs.length);
    if (next <= 4294967295) z.command("SwitchTabTo", next);
  } else if ("ScrollDown" in event) z.command("SwitchTabTo", Math.max(activeTab - 1, 1));
}

// oxlint-disable-next-line eslint/complexity -- Preserve each Wasm event's independent redraw decision.
function update(event: ReadonlyWire<Event>): boolean {
  firstRun = false;
  if (event === "SystemClipboardFailure") {
    if (isTooltip) return false;
    clipboardFailed = true;
    return true;
  }
  if (event === "InputReceived") {
    if (isTooltip) return false;
    const changed = copied !== null || clipboardFailed;
    copied = null;
    clipboardFailed = false;
    return changed;
  }
  if (typeof event === "string") return false;
  if ("InitialKeybinds" in event) {
    cachedKeybinds = event.InitialKeybinds;
    if (cachedKeybinds.length > 0) state.modeInfo = { ...state.modeInfo, keybinds: cachedKeybinds };
    return true;
  }
  if ("ModeUpdate" in event) return modeUpdate(event.ModeUpdate);
  if ("TabUpdate" in event) {
    const next = event.TabUpdate.findIndex((tab) => tab.active) + 1;
    const active = event.TabUpdate[next - 1];
    if (active) displayRows = Number(active.display_area_rows);
    if (next === 0 || next > 4294967295) return false;
    const changed = activeTab !== next || !equal(state.tabs, event.TabUpdate);
    if (isTooltip && activeTab !== next)
      z.command("BreakPanesToTabWithIndex", [[{ Plugin: ownPluginId }], next - 1, false]);
    activeTab = next;
    state.tabs = event.TabUpdate;
    return changed;
  }
  if ("PaneUpdate" in event) return paneUpdate(event.PaneUpdate);
  if ("Mouse" in event) mouse(event.Mouse);
  if ("CopyToClipboard" in event && !isTooltip) {
    const changed = copied !== event.CopyToClipboard;
    copied = event.CopyToClipboard;
    return changed;
  }
  return false;
}

function pipe(message: ReadonlyWire<PipeMessage>): boolean {
  if (isTooltip && message.is_private) {
    if (message.name === "toggle_persisted_tooltip") {
      if (firstRun) persist = true;
      else z.command("CloseSelf");
    }
  } else if (
    message.name === "toggle_tooltip" &&
    message.is_private &&
    state.toggleKey !== null &&
    ownTab === Math.max(0, activeTab - 1) &&
    message.payload === String(ownClientId)
  )
    launch(true);
  return false;
}

export default definePlugin({
  load(configuration) {
    const ids = z.getPluginIds();
    ownPluginId = ids.plugin_id;
    ownClientId = ids.client_id;
    state.hostname = ids.hostname;
    config = configuration;
    isTooltip = configuration["is_tooltip"] === "true";
    if (!isTooltip) state.toggleKey = configuration["tooltip"] ?? null;
    firstRun = isTooltip;
    z.command("SetSelectable", false);
    z.subscribe(
      isTooltip
        ? ["ModeUpdate", "TabUpdate", "InitialKeybinds"]
        : [
            "TabUpdate",
            "PaneUpdate",
            "ModeUpdate",
            "Mouse",
            "CopyToClipboard",
            "InputReceived",
            "SystemClipboardFailure",
            "InitialKeybinds",
          ],
    );
    if (state.toggleKey !== null)
      z.command("Reconfigure", [
        `
        keybinds {
            shared {
                bind "${state.toggleKey}" {
                  MessagePlugin "compact-bar" {
                      name "toggle_tooltip"
                      tooltip "${state.toggleKey}"
                      payload "${ownClientId}"
                  }
                }
            }
        }
    `,
        false,
      ]);
  },
  update,
  pipe,
  render(rows, cols) {
    if (isTooltip) return renderTooltip(state.modeInfo, rows, cols);
    if (copied !== null || clipboardFailed) {
      const hint =
        copied === "Command"
          ? "Text piped to external command"
          : copied === "Primary"
            ? "Text copied to system primary selection"
            : copied === "System"
              ? "Text copied to system clipboard"
              : " Error using the system clipboard.";
      return background(state.modeInfo, z.ui.text(hint).emphasis(2).opaque().render());
    }
    if (state.tabs.length === 0) return "";
    const output = render(state, cols);
    parts = output.parts;
    breadcrumb = output.breadcrumb;
    return background(state.modeInfo, parts.map((part) => part.part).join(""));
  },
});
