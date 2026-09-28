import z, { definePlugin, type Event, type Keybinds } from "yinz:plugin";
import { defaultMode } from "./status-bar/default-mode.ts";
import { equal, type ReadonlyWire } from "./status-bar/keys.ts";
import { render, type Clock, type Range, type RenderState } from "./status-bar/render.ts";
import { cachedTip } from "./status-bar/tips.ts";

const state: RenderState = {
  modeInfo: defaultMode(),
  tabs: [],
  classic: false,
  tipName: "quicknav",
  copied: null,
  clipboardFailure: false,
  counts: { busy: 0, waiting: 0, idle: 0 },
  clock: { time: "", date: "" },
  hintRevealed: false,
  newPaneHovered: false,
  floatingHovered: false,
};
let cachedKeybinds: ReadonlyWire<Keybinds> = [];
let altUntil = 0,
  hoverUntil = 0,
  offsetValidUntil = 0,
  utcOffset = 0;
let lastCols = 0,
  lastRows = 0;
let pendingTick: number | null = null;
let floatingRange: Range = null,
  newPaneRange: Range = null;
const hintUntil = (): number => Math.max(altUntil, hoverUntil);
const revealed = (): boolean => Date.now() < hintUntil();
const animationInterval = (): number | null =>
  !state.classic && lastRows === 1
    ? state.counts.busy
      ? 125
      : state.counts.waiting
        ? 500
        : null
    : null;
const inRange = (range: Readonly<Range>, column: number): boolean =>
  range !== null && column >= range[0] && column < range[1];

function armTick(now: number): void {
  const interval = animationInterval() ?? 60_000;
  const boundary = (Math.floor(now / interval) + 1) * interval;
  const at = hintUntil() > now ? Math.min(boundary, hintUntil()) : boundary;
  if (pendingTick === null || at < pendingTick) {
    z.command("SetTimeout", Math.max((at - now) / 1000, 0.001));
    pendingTick = at;
  }
}
function refreshClock(now: number): boolean {
  if (now >= offsetValidUntil) {
    try {
      utcOffset = z.command("GetLocalUtcOffset", { epoch_ms: now });
    } catch {
      utcOffset = 0;
    }
    offsetValidUntil = (Math.floor(now / 3_600_000) + 1) * 3_600_000;
  }
  const date = new Date(now + utcOffset * 1000);
  const clock: Clock = {
    time: `${String(date.getUTCHours()).padStart(2, "0")}:${String(date.getUTCMinutes()).padStart(2, "0")}`,
    date: `${["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][date.getUTCDay()]} ${date.getUTCDate()} ${["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][date.getUTCMonth()]}`,
  };
  const changed = !equal(clock, state.clock);
  state.clock = clock;
  return changed;
}
function notePointer(col: number): boolean {
  const now = Date.now();
  const wasRevealed = revealed();
  hoverUntil = lastCols !== 0 && col >= lastCols ? 0 : now + 30_000;
  armTick(now);
  return revealed() !== wasRevealed;
}

export default definePlugin({
  load(configuration) {
    state.tipName = cachedTip();
    state.classic = configuration["classic"] === "true";
    z.command("SetSelectable", false);
    const now = Date.now();
    refreshClock(now);
    armTick(now);
    z.subscribe([
      "Timer",
      "ModifiersHeld",
      "ModeUpdate",
      "TabUpdate",
      "PaneUpdate",
      "CopyToClipboard",
      "InputReceived",
      "SystemClipboardFailure",
      "InitialKeybinds",
      "Mouse",
    ]);
  },
  // oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Each subscribed host event updates its own state and redraw decision.
  update(event: ReadonlyWire<Event>) {
    if (event === "SystemClipboardFailure") {
      state.clipboardFailure = true;
      return true;
    }
    if (event === "InputReceived") {
      const changed = state.copied !== null || state.clipboardFailure;
      state.copied = null;
      state.clipboardFailure = false;
      return changed;
    }
    if (typeof event === "string") return false;
    if ("InitialKeybinds" in event) {
      cachedKeybinds = event.InitialKeybinds;
      if (cachedKeybinds.length > 0)
        state.modeInfo = { ...state.modeInfo, keybinds: cachedKeybinds };
      return true;
    }
    if ("ModeUpdate" in event) {
      let mi = event.ModeUpdate;
      if (mi.keybinds.length === 0 && cachedKeybinds.length > 0)
        mi = { ...mi, keybinds: cachedKeybinds };
      else if (mi.keybinds.length > 0) cachedKeybinds = mi.keybinds;
      const changed = !equal(mi, state.modeInfo);
      state.modeInfo = mi;
      return changed;
    }
    if ("TabUpdate" in event) {
      const changed = !equal(state.tabs, event.TabUpdate);
      state.tabs = event.TabUpdate;
      return changed;
    }
    if ("PaneUpdate" in event) {
      const counts = { busy: 0, waiting: 0, idle: 0 };
      for (const panes of Object.values(event.PaneUpdate.panes))
        for (const pane of panes) {
          const agent = pane.agent;
          if (!agent || agent.state === "Exited") continue;
          if (agent.state === "Idle") counts.idle += 1;
          else if (agent.state === "Waiting" && agent.wait_needs_a_person) counts.waiting += 1;
          else counts.busy += 1;
        }
      if (equal(counts, state.counts)) return false;
      state.counts = counts;
      armTick(Date.now());
      return true;
    }
    if ("ModifiersHeld" in event) {
      const now = Date.now();
      const wasRevealed = revealed();
      const { modifiers, is_live } = event.ModifiersHeld;
      if (modifiers.includes("Alt"))
        altUntil = is_live ? now + 30_000 : Math.max(altUntil, now + 3000);
      else if (is_live) altUntil = 0;
      const changed = revealed() !== wasRevealed;
      if (changed) armTick(now);
      return changed;
    }
    if ("Timer" in event) {
      const now = Date.now();
      if (pendingTick !== null && now >= pendingTick) pendingTick = null;
      const changed = refreshClock(now);
      const expired = hintUntil() !== 0 && now >= hintUntil();
      if (expired) {
        altUntil = 0;
        hoverUntil = 0;
      }
      armTick(now);
      return changed || expired || animationInterval() !== null;
    }
    if ("CopyToClipboard" in event) {
      const changed = state.copied !== event.CopyToClipboard;
      state.copied = event.CopyToClipboard;
      return changed;
    }
    if ("Mouse" in event) {
      const mouse = event.Mouse;
      if ("LeftClick" in mouse) {
        const col = Number(mouse.LeftClick[1]);
        const changed = notePointer(col);
        if (inRange(newPaneRange, col)) z.command("NewPane");
        else if (inRange(floatingRange, col)) z.command("ToggleFloatingPanes", { tab_id: null });
        return changed;
      }
      if ("Hover" in mouse) {
        const col = Number(mouse.Hover[1]);
        const changed = notePointer(col);
        const pane = inRange(newPaneRange, col);
        const floating = !pane && inRange(floatingRange, col);
        const hoverChanged = pane !== state.newPaneHovered || floating !== state.floatingHovered;
        state.newPaneHovered = pane;
        state.floatingHovered = floating;
        return changed || hoverChanged;
      }
    }
    return false;
  },
  render(rows, cols) {
    lastCols = cols;
    if (lastRows !== rows) {
      lastRows = rows;
      armTick(Date.now());
    }
    state.hintRevealed = revealed();
    const output = render(state, rows, cols, Date.now());
    newPaneRange = output.newPane;
    floatingRange = output.floating;
    return output.line.part;
  },
});
