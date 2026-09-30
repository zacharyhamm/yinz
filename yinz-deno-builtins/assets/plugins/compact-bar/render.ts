import z from "yinz:plugin";
import { keyLabel, type ModeInfo, type TabInfo } from "../status-bar/keys.ts";
import { arrow, chars, fill, line, paint, width, type Line } from "../status-bar/style.ts";
import type { Part, Range } from "../tab-bar/render.ts";

export interface Bar {
  hostname: string;
  modeInfo: ModeInfo;
  tabs: readonly TabInfo[];
  toggleKey: string | null;
  tooltipActive: boolean;
}
const length = (parts: readonly Readonly<Part>[]): number => parts.reduce((n, p) => n + p.len, 0);
const dimmed = (mi: ModeInfo): boolean =>
  mi.session_ascended === true || mi.session_dimmed === true;

function prefix(state: Readonly<Bar>, cols: number): { parts: Part[]; breadcrumb: Range } {
  const mi = state.modeInfo;
  const p = mi.style.colors.text_unselected;
  const dim = dimmed(mi);
  const style = (text: string): string => paint(text, p.base, p.background, dim ? "3" : "1");
  const host = ` ${state.hostname || "Yinz"} `;
  const parts: Part[] = [line(style(host), width(host))];
  const name = mi.style.hide_session_name ? null : mi.session_name;
  const ancestors = mi.host_fullscreen === true ? mi.session_ancestry : [];
  let breadcrumb: Range = null;
  if (name !== null) {
    if (ancestors.length > 0) {
      const ancestor = `(${ancestors.join(" ▸ ")} ▸ `;
      if (Math.max(0, cols - length(parts)) >= width(ancestor) + width(name) + 2) {
        breadcrumb = [length(parts), length(parts) + width(ancestor)];
        parts.push(
          line(
            paint(ancestor, dim ? undefined : p.base, p.background, dim ? "2" : "3"),
            width(ancestor),
          ),
          line(
            paint(name, dim ? undefined : p.emphasis_0, p.background, dim ? "2" : "1"),
            width(name),
          ),
          line(paint(") ", dim ? undefined : p.base, p.background, dim ? "2" : "1"), 2),
        );
      }
    } else if (Math.max(0, cols - length(parts)) >= width(name) + 2) {
      parts.push(line(style(`(${name})`), width(name) + 2));
    }
  }
  const mode = ` ${mi.mode.toUpperCase()} `;
  if (Math.max(0, cols - length(parts)) >= width(mode)) {
    const fg = dim
      ? p.base
      : mi.mode === "Locked"
        ? p.emphasis_3
        : mi.mode === "Normal"
          ? p.emphasis_2
          : p.emphasis_0;
    parts.push(line(paint(mode, fg, p.background, dim ? "3" : "1"), width(mode)));
  }
  return { parts, breadcrumb };
}

// Match tile-utils' scalar budget, which charges combining marks one column.
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

function tabPart(mi: ModeInfo, tab: TabInfo, alternate: boolean, budget: number): Part {
  const p = mi.style.colors;
  const r = tab.active ? p.ribbon_selected : p.ribbon_unselected;
  const dim = dimmed(mi);
  const bg = dim
    ? p.ribbon_unselected.background
    : tab.active
      ? r.background
      : alternate && mi.capabilities.arrow_fonts
        ? r.emphasis_1
        : r.background;
  const fg = dim ? p.ribbon_unselected.base : tab.is_flashing_bell ? r.emphasis_3 : r.base;
  let suffix = tab.is_fullscreen_active
    ? " (FULLSCREEN)"
    : tab.is_sync_panes_active
      ? " (SYNC)"
      : "";
  if (tab.has_bell_notification || tab.is_flashing_bell) suffix += " [!]";
  const name =
    tab.active && mi.mode === "RenameTab" && tab.name === "" ? "Enter name..." : tab.name;
  const text = ` ${ellipsize(name, Math.max(0, budget - width(suffix)))}${suffix} `;
  const style = (s: string): string => paint(s, fg, bg, dim ? "3" : "1");
  let body = style(text),
    size = width(text) + width(arrow(mi)) * 2;
  if (tab.other_focused_clients.length > 0) {
    const players = Object.fromEntries(Object.entries(p.multiplayer_user_colors));
    const cursors = tab.other_focused_clients.flatMap((id) => {
      const color = id >= 1 && id <= 10 ? players[`player_${id}`] : undefined;
      return color === undefined ? [] : [paint(" ", { EightBit: 16 }, color)];
    });
    body += style("[") + cursors.join("") + style("]");
    size += cursors.length + 2;
  }
  return {
    part:
      paint(arrow(mi), p.text_unselected.background, bg) +
      body +
      paint(arrow(mi), bg, p.text_unselected.background),
    len: size,
    tab: tab.position,
  };
}

function collapsed(mi: ModeInfo, count: number, left: boolean, tab: number): Part {
  if (count === 0) return line();
  const n = count < 10000 ? String(count) : "many";
  const text = left ? ` ← +${n} ` : ` +${n} → `;
  const p = mi.style.colors;
  const bg = p.text_selected.emphasis_0,
    outer = p.text_unselected.background;
  return {
    part:
      paint(arrow(mi), outer, bg) +
      paint(text, p.ribbon_unselected.base, bg, dimmed(mi) ? "3" : "1") +
      paint(arrow(mi), bg, outer),
    len: width(text) + width(arrow(mi)) * 2,
    tab,
  };
}

function visibleTabs(
  all: readonly Readonly<Part>[],
  active: number,
  cols: number,
  mi: ModeInfo,
): Part[] {
  const after = all.slice(active),
    before = all.slice(0, active);
  const current = after.shift() ?? before.pop() ?? line();
  const parts = [current];
  let leftWidth = 0,
    middle = current.len,
    rightWidth = 0;
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

function nestedHint(mi: ModeInfo, budget: number): Line | null {
  const ascend = mi.session_dimmed === true;
  const keys = ascend ? mi.nested_ascend_keys : mi.nested_descend_keys;
  const p = mi.style.colors.text_unselected;
  const label = ascend ? " Ascend: " : " Descend: ";
  const labels = keys.length === 0 ? ["<unbound>"] : keys.map((key) => `<${keyLabel(key)}>`);
  const size =
    width(label) + labels.reduce((n, key) => n + width(key), 0) + (labels.length - 1) * 3;
  if (size <= budget) {
    const body = labels
      .map((key) =>
        paint(
          key,
          keys.length === 0 ? undefined : p.emphasis_0,
          p.background,
          keys.length === 0 ? "2;3" : "1",
        ),
      )
      .join(paint(" + ", p.base, p.background));
    return line(paint(label, undefined, p.background, "2;3") + body, size);
  }
  const fallback = ascend ? " nested session" : " host session";
  return width(fallback) <= budget
    ? line(paint(fallback, undefined, p.background, "2;3"), width(fallback))
    : null;
}

function rightElements(state: Readonly<Bar>, active: TabInfo, budget: number): Line[] {
  const mi = state.modeInfo;
  const dim = dimmed(mi),
    p = mi.style.colors;
  if (dim) {
    const hint = nestedHint(mi, budget);
    return hint ? [hint] : [];
  }
  const parts: Line[] = [];
  if (state.toggleKey !== null) {
    const key = z.ui.text(state.toggleKey).emphasis(3).opaque();
    const ribbon = z.ui.ribbon("Tooltip");
    if (state.tooltipActive) ribbon.selected();
    parts.push(line(`${key.render()} ${ribbon.render()}`, chars(state.toggleKey) + 13));
  }
  if (mi.mode !== "RenameTab" && active.active_swap_layout_name !== null) {
    const name = ` ${active.active_swap_layout_name.replaceAll(/[a-z]/gu, (char) => char.toUpperCase())} `;
    const size = z.encodeUtf8(name).length + 3;
    if (size <= budget) {
      const outer = p.text_unselected.background;
      const bg =
        mi.mode === "Locked" || active.is_swap_layout_dirty
          ? p.ribbon_unselected.background
          : p.ribbon_selected.background;
      parts.push(
        line(
          paint(arrow(mi), outer, bg) +
            paint(name, outer, bg, mi.mode === "Locked" ? "3" : "1") +
            paint(arrow(mi), bg, outer),
          size,
        ),
      );
    }
  }
  return parts;
}

export function render(state: Readonly<Bar>, cols: number): { parts: Part[]; breadcrumb: Range } {
  const mi = state.modeInfo;
  const result = prefix(state, cols);
  const used = length(result.parts);
  const budget = Math.max(8, Math.min(40, cols - used - width(arrow(mi)) * 2 - 2));
  const all = state.tabs.map((tab, index) => tabPart(mi, tab, index % 2 === 1, budget));
  let active = state.tabs[0];
  for (const tab of state.tabs) if (tab.active) active = tab;
  if (!active) return result;
  const activePart = all[active.position] ?? all.at(-1) ?? line();
  if (used + activePart.len > cols) return result;
  result.parts.push(...visibleTabs(all, active.position, Math.max(0, cols - used), mi));
  const current = length(result.parts);
  if (current < cols) {
    const right = rightElements(state, active, cols - current);
    const rightLen = length(right);
    if (current + rightLen <= cols) {
      const bg = mi.style.colors.text_unselected.background,
        padding = cols - current - rightLen;
      if (padding > 0) result.parts.push(line(paint(" ", bg, bg).repeat(padding), padding));
      result.parts.push(...right);
    }
  }
  return result;
}

export const background = (mi: ModeInfo, text: string): string => text + fill(mi);
