import z, { type EventPayloads } from "zellij:plugin";
import {
  actionGroup,
  commonModifiers,
  keyLabel,
  keySeparator,
  modeKeymap,
  stripModifiers,
  type ModeInfo,
  type PaletteColor,
  type ReadonlyWire,
  type TabInfo,
} from "../status-bar/keys.ts";
import { arrow, concat, fill, line, paint, width, type Line } from "../status-bar/style.ts";

export type Range = [number, number] | null;
export interface Part extends Line {
  tab?: number;
}
export interface Bar {
  hostname: string;
  modeInfo: ModeInfo;
  tabs: readonly TabInfo[];
  hideSwapLayout: boolean;
  scroll: ReadonlyWire<EventPayloads["ActivePaneScroll"]>;
  hint: ReadonlyWire<EventPayloads["HintText"]> | null;
  hoveredTab: number | null;
  hoveredButton: boolean;
}
const length = (parts: readonly Readonly<Part>[]): number => parts.reduce((n, p) => n + p.len, 0);
const dimmed = (mi: ModeInfo): boolean =>
  mi.session_ascended === true || mi.session_dimmed === true;

// Match tile-utils' scalar-by-scalar budget, including its one-column charge for combining marks.
function ellipsize(text: string, budget: number): string {
  if (width(text) <= budget) return text;
  if (budget === 0) return "";
  let result = "",
    used = 0;
  for (const char of text) {
    const size = Math.max(1, width(char));
    if (used + size > budget - 1) break;
    result += char;
    used += size;
  }
  return result + "…";
}

function prefix(state: Readonly<Bar>, cols: number): { parts: Part[]; breadcrumb: Range } {
  const mi = state.modeInfo;
  const p = mi.style.colors.text_unselected;
  const dim = dimmed(mi);
  const style = (text: string): string =>
    paint(text, dim ? undefined : p.base, p.background, dim ? "2" : "1");
  const host = ` ${state.hostname || "Zellij"} `;
  const parts: Part[] = [line(style(host), width(host))];
  const name = mi.style.hide_session_name ? null : mi.session_name;
  const ancestors = mi.host_fullscreen === true ? mi.session_ancestry : [];
  if (name !== null) {
    if (ancestors.length > 0) {
      const ancestor = `(${ancestors.join(" ▸ ")} ▸ `;
      if (Math.max(0, cols - width(host)) >= width(ancestor) + width(name) + 2) {
        parts.push(
          line(
            paint(ancestor, dim ? undefined : p.base, p.background, dim ? "2" : "3"),
            width(ancestor),
          ),
          line(
            paint(name, dim ? undefined : p.emphasis_0, p.background, dim ? "2" : "1"),
            width(name),
          ),
          line(style(") "), 2),
        );
        return { parts, breadcrumb: [width(host), width(host) + width(ancestor)] };
      }
    } else if (Math.max(0, cols - width(host)) >= width(name) + 3) {
      parts.push(line(style(`(${name}) `), width(name) + 3));
    }
  }
  return { parts, breadcrumb: null };
}

function tile(mi: ModeInfo, text: string, fg: PaletteColor, bg: PaletteColor): Line {
  const outer = mi.style.colors.text_unselected.background;
  return line(
    paint(arrow(mi), outer, bg) +
      paint(text, fg, bg, dimmed(mi) ? "3" : "1") +
      paint(arrow(mi), bg, outer),
    width(text) + width(arrow(mi)) * 2,
  );
}

function tabPart(state: Readonly<Bar>, tab: TabInfo, alternate: boolean, budget: number): Part {
  const mi = state.modeInfo;
  const p = mi.style.colors;
  const r = tab.active ? p.ribbon_selected : p.ribbon_unselected;
  let bg = tab.active
    ? r.background
    : state.hoveredTab === tab.position + 1 || (alternate && mi.capabilities.arrow_fonts)
      ? r.emphasis_1
      : r.background;
  let fg = tab.is_flashing_bell ? r.emphasis_3 : r.base;
  if (dimmed(mi)) {
    bg = p.ribbon_unselected.background;
    fg = p.ribbon_unselected.base;
  }
  let suffix = tab.is_fullscreen_active
    ? " (FULLSCREEN)"
    : tab.is_sync_panes_active
      ? " (SYNC)"
      : "";
  if (tab.has_bell_notification || tab.is_flashing_bell) suffix += " [!]";
  const name =
    tab.active && mi.mode === "RenameTab" && tab.name === "" ? "Enter name..." : tab.name;
  const text = ` ${ellipsize(name, Math.max(0, budget - width(suffix)))}${suffix} `;
  const part = tile(mi, text, fg, bg);
  if (tab.other_focused_clients.length > 0) {
    const players = Object.fromEntries(Object.entries(p.multiplayer_user_colors));
    const cursors = tab.other_focused_clients.flatMap((id) => {
      const color = id >= 1 && id <= 10 ? players[`player_${id}`] : undefined;
      return color === undefined ? [] : [paint(" ", { EightBit: 16 }, color)];
    });
    const style = (s: string): string => paint(s, fg, bg, dimmed(mi) ? "3" : "1");
    part.part =
      paint(arrow(mi), p.text_unselected.background, bg) +
      style(text) +
      style("[") +
      cursors.join("") +
      style("]") +
      paint(arrow(mi), bg, p.text_unselected.background);
    part.len += cursors.length + 2;
  }
  return { ...part, tab: tab.position };
}

function collapsed(mi: ModeInfo, count: number, left: boolean, tab: number): Part {
  if (count === 0) return line();
  const n = count < 10000 ? String(count) : "many";
  const p = mi.style.colors.ribbon_unselected;
  return { ...tile(mi, left ? ` ← +${n} ` : ` +${n} → `, p.base, p.background), tab };
}

function visibleTabs(
  all: readonly Readonly<Part>[],
  active: number,
  cols: number,
  mi: ModeInfo,
): Part[] {
  const before = all.slice(0, active);
  const after = all.slice(active + 1);
  const current = all[active];
  if (!current) return [];
  const parts = [current];
  let middle = current.len;
  let leftWidth = 0;
  let rightWidth = 0;
  for (;;) {
    const left = collapsed(mi, before.length, true, Math.max(0, before.length - 1));
    const right = collapsed(mi, after.length, false, before.length + parts.length);
    const total = left.len + middle + right.len;
    if (total > cols) break;
    const nextLeft = before.at(-1),
      nextRight = after[0];
    const leftFits =
      nextLeft !== undefined && nextLeft.len + total - (before.length === 1 ? left.len : 0) <= cols;
    const rightFits =
      nextRight !== undefined &&
      nextRight.len + total - (after.length === 1 ? right.len : 0) <= cols;
    if ((leftWidth <= rightWidth || !rightFits) && leftFits) {
      before.pop();
      parts.unshift(nextLeft);
      middle += nextLeft.len;
      leftWidth += nextLeft.len;
    } else if (rightFits) {
      after.shift();
      parts.push(nextRight);
      middle += nextRight.len;
      rightWidth += nextRight.len;
    } else {
      parts.unshift(left);
      parts.push(right);
      break;
    }
  }
  return parts;
}

function swapKeys(mi: ModeInfo): Line {
  const keys = actionGroup(modeKeymap(mi), [["PreviousSwapLayout"], ["NextSwapLayout"]]);
  if (keys.length === 0) return line();
  const common = commonModifiers(keys);
  const labels = keys.map((key) => keyLabel(keys.length > 1 ? stripModifiers(key, common) : key));
  const joined = labels.join(keySeparator(labels));
  const modifier = common.join("-");
  const multi = common.length > 0 && keys.length > 1;
  const content = multi ? ` ${modifier} <${joined}> ` : ` ${joined} `;
  const text = z.ui.text(content).opaque();
  if (dimmed(mi)) text.disabled();
  else if (multi)
    text
      .emphasis(0, 0, width(modifier) + 1)
      .emphasis(0, width(modifier) + 3, Math.min(text.len(), width(modifier) + 3 + width(joined)));
  else text.emphasis(0);
  return line(text.render(), width(content));
}

function indicator(state: Readonly<Bar>, active: TabInfo): Line | null {
  const mi = state.modeInfo;
  if (!state.hideSwapLayout && active.active_swap_layout_name !== null) {
    const name = active.active_swap_layout_name.toUpperCase();
    const ribbon = z.ui.ribbon(name);
    if (dimmed(mi)) ribbon.disabled();
    else if (!active.is_swap_layout_dirty) ribbon.selected();
    return concat(swapKeys(mi), line(ribbon.render(), width(name) + 2 + width(arrow(mi)) * 2));
  }
  if (state.scroll !== null) {
    const text = `[ SCROLL ${state.scroll[0]}/${state.scroll[1]} ]`;
    const p = mi.style.colors.text_unselected;
    return line(paint(text, p.base, p.background, dimmed(mi) ? "3" : "1"), width(text));
  }
  return null;
}

function hintPart(state: Readonly<Bar>, budget: number): Line | null {
  if (state.hint === null || state.modeInfo.pane_frame_style === "full") return null;
  const best = Object.entries(state.hint)
    .filter(([size]) => Number(size) <= budget)
    .sort(([a], [b]) => Number(b) - Number(a))[0]?.[1];
  if (!best) return null;
  const text = z.ui.text(best.text);
  for (const [level, indices] of best.indices.entries())
    for (const index of indices)
      if (level <= 8 && Number(index) < text.len())
        text.emphasis(level, Number(index), Number(index) + 1);
  if (dimmed(state.modeInfo)) text.disabled();
  return line(text.render(), width(best.text));
}

export function render(
  state: Readonly<Bar>,
  cols: number,
): { parts: Part[]; button: Range; breadcrumb: Range } {
  const mi = state.modeInfo;
  const { parts, breadcrumb } = prefix(state, cols);
  const budget = Math.max(8, Math.min(40, cols - length(parts) - width(arrow(mi)) * 4 - 5));
  const all = state.tabs.map((tab, index) => tabPart(state, tab, index % 2 === 1, budget));
  const active = state.tabs.find((tab) => tab.active);
  const activePart = active && all[active.position];
  if (!active || !activePart) return { parts: [], button: null, breadcrumb: null };
  let status =
    hintPart(state, Math.max(0, cols - length(parts) - activePart.len)) ?? indicator(state, active);
  if (status !== null && status.len > 0 && length(parts) + activePart.len + status.len > cols)
    status = null;
  parts.push(
    ...visibleTabs(
      all,
      active.position,
      Math.max(0, cols - length(parts) - (status?.len ?? 0)),
      mi,
    ),
    line(fill(mi), 0),
  );
  const p = mi.style.colors.ribbon_unselected;
  const alternate = mi.capabilities.arrow_fonts ? state.tabs.length % 2 === 1 : state.hoveredButton;
  const buttonPart = tile(
    mi,
    " + ",
    p.base,
    alternate && !dimmed(mi) ? p.emphasis_1 : p.background,
  );
  const used = length(parts);
  let button: Range = null;
  if (Math.max(0, cols - used - (status?.len ?? 0)) >= buttonPart.len) {
    button = [used, used + buttonPart.len];
    parts.push(buttonPart);
  }
  if (status !== null) {
    const padding = Math.max(0, cols - length(parts) - status.len);
    const bg = mi.style.colors.text_unselected.background;
    parts.push(concat(line(paint(" ".repeat(padding), bg, bg), padding), status));
  }
  parts.push(line(fill(mi), 0));
  return { parts, button, breadcrumb };
}
