import z, { type InputMode } from "zellij:plugin";
import type { ModeInfo, TabInfo, ReadonlyWire } from "./keys.ts";
import {
  actionGroup,
  actionKeys,
  charKey,
  commonModifiers,
  directionAction as dir,
  equal,
  genericKey,
  keyLabel,
  modeKeymap,
  newPane,
  resize,
  stripModifiers,
  switchMode,
  toChar,
  type Key,
} from "./keys.ts";
import { renderHints } from "./hints.ts";
import {
  abbreviatedKey,
  arrow,
  byteLine,
  chars,
  classicKeys,
  concat,
  fill,
  line,
  modifierPrefix,
  paint,
  prefixPaint,
  prefixSuffix,
  shortcut,
  textPart,
  tileColors,
  width,
  type Line,
  type TileMode,
} from "./style.ts";
import { tip } from "./tips.ts";

export interface Counts {
  busy: number;
  waiting: number;
  idle: number;
}
export interface Clock {
  time: string;
  date: string;
}
export type Range = [number, number] | null;
export interface RenderState {
  modeInfo: ModeInfo;
  tabs: readonly TabInfo[];
  classic: boolean;
  tipName: string;
  copied: string | null;
  clipboardFailure: boolean;
  counts: Counts;
  clock: Clock;
  hintRevealed: boolean;
  newPaneHovered: boolean;
  floatingHovered: boolean;
}
interface RibbonLine {
  line: Line;
  newPane: Range;
  floating: Range;
}
interface Tile {
  mode: TileMode;
  action: TileAction;
  key: Key | undefined;
}
const labels = {
  Lock: ["LOCK", "Lo"],
  Unlock: ["UNLOCK", "Un"],
  Pane: ["PANE", "Pa"],
  Tab: ["TAB", "Ta"],
  Resize: ["SIZE", "Re"],
  Search: ["SEARCH", "Se"],
  Quit: ["QUIT", "Qu"],
  Session: ["SESS", "Se"],
  Move: ["MOVE", "Mo"],
  Tmux: ["TMUX", "Tm"],
  Agent: ["AGENT", "Ag"],
  Notifications: ["NOTIFY", "No"],
} as const;
type TileAction = keyof typeof labels;
const modeTiles: [InputMode | "Quit", TileAction][] = [
  ["Locked", "Lock"],
  ["Agent", "Agent"],
  ["Pane", "Pane"],
  ["Tab", "Tab"],
  ["Resize", "Resize"],
  ["Move", "Move"],
  ["Scroll", "Search"],
  ["Session", "Session"],
  ["Quit", "Quit"],
];
const selectedAction: Partial<Record<InputMode, TileAction>> = {
  Locked: "Lock",
  Pane: "Pane",
  RenamePane: "Pane",
  Select: "Pane",
  Tab: "Tab",
  RenameTab: "Tab",
  Resize: "Resize",
  Move: "Move",
  Scroll: "Search",
  Search: "Search",
  EnterSearch: "Search",
  Session: "Session",
  Agent: "Agent",
};
const bound = (mi: ModeInfo, mode: InputMode, target: InputMode | "Quit"): Key | undefined =>
  toChar(actionKeys(modeKeymap(mi, mode), [target === "Quit" ? "Quit" : switchMode(target)]));
const noRanges = (l: Readonly<Line>): RibbonLine => ({ line: l, newPane: null, floating: null });
const shift = (r: Readonly<Range>, n: number): Range => r && [r[0] + n, r[1] + n];

function classicTile(
  mi: ModeInfo,
  tile: Readonly<Tile>,
  common: Key["key_modifiers"],
  first: boolean,
  form: number,
): Line {
  if (!tile.key && tile.mode !== "disabled") return line();
  const key = tile.key
    ? form === 1
      ? abbreviatedKey(tile.key, common)
      : keyLabel(stripModifiers(tile.key, common))
    : "";
  const name = labels[tile.action][0];
  const c = tileColors(mi, tile.mode);
  const start = common.length === 0 && first ? "" : arrow(mi);
  const end = arrow(mi);
  const body =
    form === 2
      ? c.key(` ${key} `)
      : c.bracket(" <") + c.key(key) + c.bracket("> ") + c.text(`${name} `);
  return line(c.prefix(start) + body + c.suffix(end));
}
// oxlint-disable-next-line eslint/max-lines-per-function -- Keep the classic tile fitting and swap-layout alignment in their rendering order.
function firstLine(mi: ModeInfo, tab: TabInfo | undefined, maxCols: number): Line {
  const max = Math.min(maxCols, 10_000);
  const tiles: Tile[] = modeTiles.map(([target, action], i) => ({
    mode: i % 2 ? "alternate" : "unselected",
    action,
    key: bound(mi, mi.mode, target),
  }));
  const selected = tiles.find((t) => t.action === selectedAction[mi.mode]);
  if (selected) {
    selected.mode = "selected";
    selected.key = bound(mi, mi.mode, "Normal");
  }
  if (mi.mode === "Locked") for (const tile of tiles.slice(1)) tile.mode = "disabled";
  if (mi.mode === "Tmux" || mi.mode === "Notifications")
    tiles.push({ mode: "selected", action: mi.mode, key: bound(mi, mi.mode, "Normal") });
  const switchKeys = modeKeymap(mi)
    .filter(
      ([k, aa]) =>
        !genericKey(k) &&
        (aa[0] === "Quit" ||
          (aa[0] !== undefined &&
            typeof aa[0] !== "string" &&
            "SwitchToMode" in aa[0] &&
            [
              "Normal",
              "Locked",
              "Pane",
              "Tab",
              "Resize",
              "Move",
              "Scroll",
              "Session",
              "Agent",
            ].includes(aa[0].SwitchToMode.input_mode))),
    )
    .map(([k]) => k);
  const common = commonModifiers(switchKeys);
  const prefix =
    common.length > 0
      ? line(prefixPaint(mi, modifierPrefix(mi, common)) + prefixSuffix(mi))
      : line();
  let result = line();
  for (let form = 0; form < 3; form++) {
    let candidate = prefix;
    for (const tile of tiles)
      candidate = concat(candidate, classicTile(mi, tile, common, candidate.len === 0, form));
    if (candidate.len < max) {
      result = candidate;
      break;
    }
  }
  if (result.len < max && tab?.active_swap_layout_name != null) {
    const name = ` ${tab.active_swap_layout_name.replaceAll(/[a-z]/gu, (c) => c.toUpperCase())} `;
    const c = tileColors(
      mi,
      mi.mode === "Locked" ? "disabled" : tab.is_swap_layout_dirty ? "unselected" : "selected",
    );
    const indicator = line(c.prefix(arrow(mi)) + c.text(name) + c.suffix(arrow(mi)));
    const keys = byteLine(
      classicKeys(
        actionGroup(modeKeymap(mi), [["PreviousSwapLayout"], ["NextSwapLayout"]]),
        mi.style.colors,
        mi.style.colors.text_unselected.background,
      ),
    );
    const space = line(prefixPaint(mi, " "));
    const full = mi.mode === "Locked" ? indicator : concat(keys, space, indicator, space);
    const available = max - result.len;
    const fitted =
      full.len <= available
        ? full
        : mi.mode !== "Locked" && indicator.len <= available
          ? indicator
          : undefined;
    if (fitted)
      result = concat(result, line(prefixPaint(mi, " ".repeat(available - fitted.len))), fitted);
  }
  return result;
}

// oxlint-disable-next-line eslint/max-lines-per-function -- Keep the per-mode ribbon table and its shared modifier selection together.
function modeIndicators(mi: ModeInfo, max: number): Line | undefined {
  const locked = mi.base_mode === "Locked";
  const all = new Map<InputMode, Tile[]>([
    [
      "Locked",
      [
        {
          mode: locked ? "unselected" : "selected",
          action: locked ? "Unlock" : "Lock",
          key: bound(mi, "Locked", "Normal"),
        },
      ],
    ],
    [
      "Normal",
      modeTiles.map(([target, action]) => ({
        mode:
          target === "Locked" && locked
            ? "selected"
            : ["Agent", "Pane", "Resize", "Scroll"].includes(target)
              ? "alternate"
              : "unselected",
        action: target === "Locked" && locked ? "Unlock" : action,
        key: bound(mi, "Normal", target),
      })),
    ],
  ]);
  for (const [mode, action] of [
    ["Agent", "Agent"],
    ["Pane", "Pane"],
    ["Notifications", "Notifications"],
    ["Select", "Pane"],
    ["Tab", "Tab"],
    ["Resize", "Resize"],
    ["Move", "Move"],
    ["Scroll", "Search"],
    ["Session", "Session"],
  ] as [InputMode, TileAction][]) {
    const tiles: Tile[] = [];
    if (locked || mode === "Notifications")
      tiles.push({ mode: "selected", action: "Unlock", key: bound(mi, mode, "Locked") });
    tiles.push({ mode: "selected", action, key: bound(mi, mode, "Normal") });
    all.set(mode, tiles);
  }
  const common = commonModifiers(
    [...all.values()].flat().flatMap((t) => (t.key && !genericKey(t.key) ? [t.key] : [])),
  );
  const tiles = all.get(mi.mode);
  let result = line();
  if (tiles) {
    if (common.length > 0)
      result = concat(
        textPart(modifierPrefix(mi, common), undefined, mi.session_dimmed ?? false),
        line(prefixSuffix(mi)),
      );
    for (const short of [false, true]) {
      const list = concat(
        ...tiles.map((t) =>
          shortcut(
            mi,
            labels[t.action][short ? 1 : 0],
            t.key ? [common.length > 0 ? stripModifiers(t.key, common) : t.key] : [],
            t.mode === "selected",
            3,
            common.length > 0,
            false,
            common.length > 0 && short,
          ),
        ),
      );
      if (result.len + list.len <= max) {
        result = concat(result, list);
        break;
      }
    }
  }
  return result.len <= max ? result : undefined;
}

function groupSeparator(mi: ModeInfo, max: number): Line | undefined {
  const name = (
    {
      RenamePane: "RENAMING PANE",
      RenameTab: "RENAMING TAB",
      EnterSearch: "ENTERING SEARCH TERM",
      Search: "SEARCHING",
    } as Partial<Record<InputMode, string>>
  )[mi.mode];
  const p = mi.style.colors;
  const sep = arrow(mi) || " ";
  const c = p.text_unselected.emphasis_0;
  const bg = p.ribbon_selected.base;
  const result = line(
    (name ? paint(` ${name} `, c, bg, "1") : "") +
      paint(sep, bg, c, "1") +
      paint(" ", c, c, "1") +
      paint(sep, c, bg, "1"),
  );
  return result.len <= max ? result : undefined;
}

const choose = (kk: readonly Key[], prefer: (k: Key) => boolean, fallback = kk[0]): Key[] => {
  const k = kk.find((candidate) => prefer(candidate)) ?? fallback;
  return k ? [k] : [];
};

// oxlint-disable-next-line eslint/max-lines-per-function -- Fit the navigation hints as a group and record their final clickable ranges.
function secondaryHints(
  mi: ModeInfo,
  tab: TabInfo | undefined,
  max: number,
  state: ReadonlyWire<RenderState>,
): RibbonLine | undefined {
  const km = modeKeymap(mi);
  const pane = choose(actionKeys(km, [newPane()]), (k) => equal(k, charKey("n", ["Alt"])));
  const increase = actionKeys(km, [resize("Increase")]);
  const sizes = [
    ...choose(increase, (k) => equal(k.bare_key, { Char: "+" })),
    ...choose(
      actionKeys(km, [resize("Decrease")]),
      (k) => equal(k.bare_key, { Char: "-" }),
      increase[0],
    ),
  ];
  const focus = ["Left", "Down", "Up", "Right"].flatMap((d) =>
    choose(
      actionKeys(km, [dir(d === "Left" || d === "Right" ? "MoveFocusOrTab" : "MoveFocus", d)]),
      (k) => k.bare_key === d,
    ),
  );
  const floating = choose(actionKeys(km, ["ToggleFloatingPanes"]), (k) =>
    equal(k, charKey("f", ["Alt"])),
  );
  const common = commonModifiers([...pane, ...focus, ...sizes, ...floating]);
  const multi =
    tab &&
    Number(
      tab.are_floating_panes_visible
        ? tab.selectable_floating_panes_count
        : tab.selectable_tiled_panes_count,
    ) > 1;
  for (const short of [false, true]) {
    let result =
      common.length > 0
        ? textPart(`${common.join("-")} + `, 0, mi.session_dimmed ?? false)
        : line();
    const append = (
      label: string,
      keys: readonly Key[],
      selected = false,
      hovered = false,
    ): Range => {
      const start = result.len;
      result = concat(
        result,
        shortcut(
          mi,
          label,
          common.length > 0 ? keys.map((k) => stripModifiers(k, common)) : keys,
          selected,
          0,
          common.length > 0,
          hovered,
        ),
      );
      return result.len > start ? [start, result.len] : null;
    };
    const paneRange = append(short ? "New" : "New Pane", pane, false, state.newPaneHovered);
    if (multi) {
      append(short ? "Focus" : "Change Focus", focus);
      append("Resize", sizes);
    }
    const floatingRange = append(
      "Floating",
      floating,
      tab?.are_floating_panes_visible,
      state.floatingHovered,
    );
    if (result.len <= max)
      return result.len ? { line: result, newPane: paneRange, floating: floatingRange } : undefined;
  }
  return undefined;
}

// oxlint-disable-next-line eslint/max-lines-per-function -- Try the reference status variants in order, shedding date, idle agents and all agents.
function statusBlock(
  mi: ModeInfo,
  counts: Readonly<Counts>,
  clock: Readonly<Clock>,
  max: number,
  now: number,
): Line {
  const frame = ((Math.floor(now / 125) % 8) + 8) % 8;
  const simple = mi.capabilities.arrow_fonts;
  const mark = (glyph: string, count: number): string =>
    simple ? `${count}${glyph}` : `${glyph}${count}`;
  for (const [agents, idle, date] of [
    [true, true, true],
    [true, true, false],
    [true, false, false],
    [false, false, false],
  ]) {
    const segments: [string, "dim" | "emphasis" | "alert"][] = [];
    if (agents) {
      const add = (text: string, style: "dim" | "emphasis" | "alert"): void => {
        if (segments.length > 0) segments.push([" ", "dim"]);
        segments.push([text, style]);
      };
      if (counts.waiting)
        add(
          mark(simple ? "w" : "◆", counts.waiting),
          Math.floor(now / 500) % 2 === 0 ? "alert" : "dim",
        );
      if (counts.busy)
        add(
          mark(
            simple
              ? (["|", "/", "-", "\\"][frame % 4] ?? "|")
              : (["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"][frame] ?? "⣾"),
            counts.busy,
          ),
          "emphasis",
        );
      if (idle && counts.idle) add(mark(simple ? "i" : "·", counts.idle), "dim");
    }
    if (segments.length > 0) segments.push(["   ", "dim"]);
    segments.push([clock.time, "emphasis"]);
    if (date) segments.push(["  ", "dim"], [clock.date, "dim"]);
    const len = segments.reduce((n, [text]) => n + width(text), 0);
    if (len <= max)
      return line(
        segments
          .map(([text, style]) => {
            const t = z.ui.text(text).opaque();
            if (mi.session_dimmed) t.disabled();
            else if (style === "dim") t.dim();
            else if (style === "alert") t.error();
            else t.emphasis(1);
            return t.render();
          })
          .join(""),
        len,
      );
  }
  return line();
}

function message(state: ReadonlyWire<RenderState>, max: number): Line | undefined {
  const mi = state.modeInfo;
  const p = mi.style.colors;
  if (mi.session_dimmed || mi.session_ascended) {
    const kk = mi.session_dimmed ? mi.nested_ascend_keys : mi.nested_descend_keys;
    const keys =
      kk.length > 0
        ? kk.map((k) => classicKeys([k], p)).join(paint(" + ", p.text_unselected.base))
        : paint("<unbound>", undefined, undefined, "2;3");
    const full = line(
      paint(mi.session_dimmed ? " Ascend: " : " Descend: ", undefined, undefined, "2;3") + keys,
    );
    return full.len <= max ? full : line(keys).len <= max ? line(keys) : line();
  }
  if (state.copied)
    return textPart(
      state.copied === "Command"
        ? "Text piped to external command"
        : state.copied === "Primary"
          ? "Text copied to system primary selection"
          : "Text copied to system clipboard",
      2,
    );
  if (state.clipboardFailure)
    return line(
      paint(" Error using the system clipboard.", p.text_unselected.emphasis_3, undefined, "1"),
    );
  return undefined;
}

function secondLine(state: ReadonlyWire<RenderState>, tab: TabInfo | undefined, max: number): Line {
  const mi = state.modeInfo;
  const c = mi.style.colors.text_unselected;
  const notification = message(state, max);
  if (notification) return notification;
  if (!tab) return line();
  const base = (s: string): string => paint(s, c.base, undefined, "1");
  const orange = (s: string): string => paint(s, c.emphasis_0, undefined, "1");
  const green = (s: string): string => paint(s, c.emphasis_2, undefined, "1");
  const locked = mi.mode === "Locked" ? base(" -- INTERFACE LOCKED -- ") : "";
  if (mi.mode === "Locked" || mi.mode === "Normal") {
    if (tab.is_fullscreen_active)
      return line(
        locked +
          base(" (") +
          orange("FULLSCREEN") +
          base("): + ") +
          green(String(tab.panes_to_hide)) +
          base(" hidden panes"),
      );
    if (tab.are_floating_panes_visible) {
      const prefix = locked + base(" (") + orange("FLOATING PANES VISIBLE");
      if (mi.mode === "Locked") return line(prefix + base(")"));
      const paneKey = actionKeys(modeKeymap(mi), [switchMode("Pane")])[0] ?? charKey("?");
      const toggle =
        actionKeys(modeKeymap(mi, "Pane"), ["ToggleFloatingPanes", switchMode("Normal")])[0] ??
        charKey("?");
      return line(
        prefix +
          base("): Press ") +
          green(keyLabel(paneKey)) +
          base(", <") +
          green(keyLabel(toggle)) +
          base("> to hide."),
      );
    }
    if (mi.mode === "Normal") return tip(mi, state.tipName, max);
    return chars(" -- INTERFACE LOCKED -- ") <= max ? line(locked) : line();
  }
  return renderHints(mi, max, (first, label, keys) =>
    keys.length > 0
      ? line(
          paint(first ? " " : " / ", c.base) +
            classicKeys(keys, mi.style.colors) +
            base(` ${label}`),
        )
      : line(),
  );
}

// oxlint-disable-next-line eslint/max-lines-per-function -- Keep the classic and single-line layout selection and alignment together.
export function render(
  state: ReadonlyWire<RenderState>,
  rows: number,
  cols: number,
  now: number,
): RibbonLine {
  const mi = state.modeInfo;
  const tab = state.tabs.find((t) => t.active);
  if (rows !== 1 || state.classic) {
    const first = firstLine(mi, tab, cols);
    const second = secondLine(state, tab, cols);
    return noRanges(
      line(
        rows > 1
          ? first.part + fill(mi) + "\n\u001B[m" + second.part + "\u001B[0K"
          : mi.session_dimmed || mi.session_ascended || mi.mode !== "Normal"
            ? "\u001B[m" + second.part + "\u001B[0K"
            : first.part + fill(mi),
      ),
    );
  }
  const notification = message(state, cols);
  if (notification) return noRanges(line(notification.part + fill(mi)));
  let result = noRanges(line());
  const left = modeIndicators(mi, cols);
  if (left) {
    result.line = left;
    const remaining = Math.max(0, cols - left.len);
    if (mi.mode === "Normal" || mi.mode === "Locked") {
      const secondary =
        (state.hintRevealed ? secondaryHints(mi, tab, remaining, state) : undefined) ??
        noRanges(statusBlock(mi, state.counts, state.clock, remaining, now));
      const padding = Math.max(0, remaining - secondary.line.len - 1);
      result = {
        line: concat(
          left,
          line(prefixPaint(mi, " ".repeat(padding), mi.session_dimmed ?? false)),
          secondary.line,
        ),
        newPane: shift(secondary.newPane, left.len + padding),
        floating: shift(secondary.floating, left.len + padding),
      };
    } else {
      const separator = groupSeparator(mi, remaining);
      if (separator)
        result.line = concat(
          left,
          separator,
          renderHints(mi, remaining - separator.len, (_first, label, keys) =>
            shortcut(mi, label, keys, false, 2),
          ),
        );
    }
  }
  result.line.part += fill(mi);
  return result;
}
