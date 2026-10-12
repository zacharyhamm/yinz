import z, { definePlugin, type Event, type Keybinds, type Mouse } from "yinz:plugin";
import { defaultMode } from "./status-bar/default-mode.ts";
import { equal, type ReadonlyWire } from "./status-bar/keys.ts";
import { render, type Bar, type Part, type Range } from "./tab-bar/render.ts";

const state: Bar = {
  hostname: "",
  modeInfo: defaultMode(),
  tabs: [],
  hideSwapLayout: false,
  scroll: null,
  hint: null,
  hoveredTab: null,
  hoveredButton: false,
};
let activeTab = 0;
let cachedKeybinds: ReadonlyWire<Keybinds> = [];
let outstandingHints = 0;
let parts: Part[] = [];
let newTabRange: Range = null;
let breadcrumbRange: Range = null;
const inRange = (range: Readonly<Range>, col: number): boolean =>
  range !== null && col >= range[0] && col < range[1];

function tabAt(col: number): number | null {
  let start = 0;
  for (const part of parts) {
    if (col >= start && col < start + part.len) {
      const tab = part.tab === undefined ? null : part.tab + 1;
      return tab !== activeTab && tab !== null && tab <= 4294967295 ? tab : null;
    }
    start += part.len;
  }
  return null;
}

function mouse(event: ReadonlyWire<Mouse>): boolean {
  if ("LeftClick" in event) {
    const col = Number(event.LeftClick[1]);
    if (inRange(breadcrumbRange, col)) z.command("FocusHostSession");
    else if (inRange(newTabRange, col)) z.command("NewTab", { name: null, cwd: null });
    else {
      const tab = tabAt(col);
      if (tab !== null) z.command("SwitchTabTo", tab);
    }
  } else if ("Hover" in event) {
    const col = Number(event.Hover[1]);
    const button = !state.modeInfo.capabilities.arrow_fonts && inRange(newTabRange, col);
    const tab = !state.modeInfo.capabilities.arrow_fonts && !button ? tabAt(col) : null;
    const changed = button !== state.hoveredButton || tab !== state.hoveredTab;
    state.hoveredButton = button;
    state.hoveredTab = tab;
    return changed;
  } else if ("ScrollUp" in event) {
    const next = Math.min(activeTab + 1, state.tabs.length);
    if (next <= 4294967295) z.command("SwitchTabTo", next);
  } else if ("ScrollDown" in event) z.command("SwitchTabTo", Math.max(activeTab - 1, 1));
  return false;
}

// oxlint-disable-next-line eslint/max-lines-per-function -- Each subscribed host event preserves the Wasm state transition and redraw decision.
function update(event: ReadonlyWire<Event>): boolean {
  if (event === "InputReceived") {
    const changed = state.hint !== null;
    state.hint = null;
    return changed;
  }
  if (typeof event === "string") return false;
  if ("InitialKeybinds" in event) {
    cachedKeybinds = event.InitialKeybinds;
    if (cachedKeybinds.length > 0) state.modeInfo = { ...state.modeInfo, keybinds: cachedKeybinds };
    return true;
  }
  if ("ModeUpdate" in event) {
    let mode = event.ModeUpdate;
    if (mode.keybinds.length === 0 && cachedKeybinds.length > 0)
      mode = { ...mode, keybinds: cachedKeybinds };
    else if (mode.keybinds.length > 0) cachedKeybinds = mode.keybinds;
    const changed = !equal(state.modeInfo, mode);
    state.modeInfo = mode;
    return changed;
  }
  if ("TabUpdate" in event) {
    const next = event.TabUpdate.findIndex((tab) => tab.active) + 1;
    if (next === 0 || next > 4294967295) return false;
    const changed = activeTab !== next || !equal(state.tabs, event.TabUpdate);
    activeTab = next;
    state.tabs = event.TabUpdate;
    return changed;
  }
  if ("ActivePaneScroll" in event) {
    const changed = !equal(state.scroll, event.ActivePaneScroll);
    state.scroll = event.ActivePaneScroll;
    return changed;
  }
  if ("HintText" in event) {
    if (Object.keys(event.HintText).length === 0) {
      const changed = state.hint !== null;
      state.hint = null;
      return changed;
    }
    state.hint = event.HintText;
    outstandingHints += 1;
    z.command("SetTimeout", 5);
    return true;
  }
  if ("Timer" in event) {
    outstandingHints = Math.max(0, outstandingHints - 1);
    if (outstandingHints === 0 && state.hint !== null) {
      state.hint = null;
      return true;
    }
  }
  if ("Mouse" in event) return mouse(event.Mouse);
  return false;
}

export default definePlugin({
  load(configuration) {
    state.hostname = z.getPluginIds().hostname;
    state.hideSwapLayout = configuration["hide_swap_layout_indication"] === "true";
    z.command("SetSelectable", false);
    z.subscribe([
      "TabUpdate",
      "ModeUpdate",
      "Mouse",
      "InitialKeybinds",
      "ActivePaneScroll",
      "HintText",
      "Timer",
      "InputReceived",
    ]);
  },
  update(event: ReadonlyWire<Event>) {
    const changed = update(event);
    return state.tabs.length > 0 && changed;
  },
  render(_rows, cols) {
    if (state.tabs.length === 0) return "";
    const output = render(state, Math.max(0, cols - 1));
    parts = output.parts;
    newTabRange = output.button;
    breadcrumbRange = output.breadcrumb;
    return parts.map((p) => p.part).join("");
  },
});
