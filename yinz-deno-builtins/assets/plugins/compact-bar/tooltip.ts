import z, { type InputMode } from "yinz:plugin";
import {
  directionAction,
  equal,
  keyLabel,
  modeKeymap,
  pluginKeys,
  resize,
  switchMode,
  tag,
  type Action,
  type ModeInfo,
  type Pattern,
} from "../status-bar/keys.ts";
import { chars } from "../status-bar/style.ts";

type Predicate = (action: Action) => boolean;
const matches =
  (pattern: Pattern): Predicate =>
  (action) => {
    if (typeof pattern === "string") return tag(action) === pattern;
    if (typeof action === "string" || tag(action) !== tag(pattern)) return false;
    const target = Object.values(pattern)[0];
    const value: unknown = Object.values(action)[0];
    return (
      typeof target === "object" &&
      target !== null &&
      typeof value === "object" &&
      value !== null &&
      Object.entries(target).every(([key, expected]) =>
        equal(Object.fromEntries(Object.entries(value))[key], expected),
      )
    );
  };
const launches =
  (alias: string): Predicate =>
  (action) =>
    pluginKeys([[{ bare_key: "Enter", key_modifiers: [] }, [action]]], alias).length > 0;
const directions = ["Left", "Down", "Up", "Right"];
const switches = (...modes: readonly InputMode[]): Predicate[] =>
  modes.map((mode) => matches(switchMode(mode)));
const named = (...names: readonly string[]): Predicate[] => names.map((name) => matches(name));
// Wasm's action payload omits start_suppressed and all NewStackedPane fields.
const newPane = (direction: string | null = null): Pattern => ({
  NewPane: { direction, pane_name: null },
});

// The order is the compact Wasm bar's curated action list, including predicates
// that deliberately group all directions under the first matching action type.
// oxlint-disable-next-line eslint/max-lines-per-function -- Keep each mode's curated action order together, matching the Wasm tooltip.
function predicates(mode: InputMode): Predicate[] {
  switch (mode) {
    case "Locked":
      return switches("Normal");
    case "Normal":
      return [
        ...switches("Locked", "Pane", "Tab", "Resize", "Move", "Scroll", "Session", "Agent"),
        ...named("Quit"),
      ];
    case "Pane":
      return [
        matches(newPane()),
        ...directions.map((direction) => matches(directionAction("MoveFocus", direction))),
        ...named("CloseFocus"),
        ...switches("RenamePane"),
        ...named("ToggleFocusFullscreen", "ToggleFloatingPanes", "TogglePaneEmbedOrFloating"),
        ...named("NewStackedPane"),
        matches(newPane("Right")),
        matches(newPane("Down")),
      ];
    case "Tab":
      return [
        ...named("GoToPreviousTab", "GoToNextTab"),
        matches({
          NewTab: {
            layout_source: null,
            tiled_layout: null,
            swap_tiled_layouts: null,
            swap_floating_layouts: null,
            tab_name: null,
            should_change_focus_to_new_tab: true,
            cwd: null,
          },
        }),
        ...named("CloseTab"),
        ...switches("RenameTab"),
        ...named(
          "TabNameInput",
          "ToggleActiveSyncTab",
          "BreakPane",
          "BreakPaneLeft",
          "BreakPaneRight",
          "ToggleTab",
        ),
      ];
    case "Resize":
      return [
        matches(resize("Increase")),
        matches(resize("Decrease")),
        ...["Increase", "Decrease"].flatMap((size) =>
          directions.map((direction) => matches(resize(size, direction))),
        ),
      ];
    case "Move":
      return directions.map((direction) => matches(directionAction("MovePane", direction)));
    case "Scroll":
      return [
        ...named(
          "ScrollDown",
          "ScrollUp",
          "HalfPageScrollDown",
          "HalfPageScrollUp",
          "PageScrollDown",
          "PageScrollUp",
        ),
        ...switches("EnterSearch"),
        ...named("EditScrollback"),
      ];
    case "Search":
      return [
        ...switches("EnterSearch"),
        ...named(
          "SearchInput",
          "ScrollDown",
          "ScrollUp",
          "PageScrollDown",
          "PageScrollUp",
          "HalfPageScrollDown",
          "HalfPageScrollUp",
        ),
        ...["Down", "Up"].map((direction) => matches(directionAction("Search", direction))),
        ...["CaseSensitivity", "Wrap", "WholeWord"].map((option) =>
          matches({ SearchToggleOption: { option } }),
        ),
      ];
    case "Select":
      return [
        ...directions.map((direction) => matches(directionAction("MoveTextSelection", direction))),
        ...named("SwapTextSelectionEnds"),
      ];
    case "Session":
      return [
        ...named("Detach"),
        ...["session-manager", "plugin-manager", "configuration", "yinz:about"].map((alias) =>
          launches(alias),
        ),
      ];
    case "Notifications":
      return [
        ...["Down", "Up"].map((direction) =>
          matches(directionAction("MoveNotificationSelection", direction)),
        ),
        ...named("ActivateNotification", "ToggleNotificationDetail"),
      ];
    case "Agent":
      return ["hocus-new-agent", "hocus-agents", "hocus"].map((alias) => launches(alias));
    case "EnterSearch":
    case "Prompt":
    case "RenamePane":
    case "RenameTab":
    case "Tmux":
      return [];
    default:
      return [];
  }
}

const descriptions: Readonly<Record<string, string>> = {
  MoveFocus: "Move focus",
  MovePaneWithDirection: "Move pane",
  MovePaneWithoutDirection: "Move pane",
  ResizeIncrease: "Increase size in direction",
  ResizeDecrease: "Decrease size in direction",
  ResizeAny: "Increase or decrease size",
  Search: "Search",
  NewPaneWithDirection: "Split right/down",
  NewPaneWithoutDirection: "New pane",
  BreakPaneLeftOrRight: "Break pane to adjacent tab",
  GoToAdjacentTab: "Move tab focus",
  Scroll: "Scroll",
  PageScroll: "Scroll page",
  HalfPageScroll: "Scroll half Page",
  SessionManager: "Session manager",
  PluginManager: "Plugin manager",
  Configuration: "Configuration",
  About: "About Yinz",
  TogglePaneEmbedOrFloating: "Float or embed",
  NewStackedPane: "New stacked pane",
  ToggleFocusFullscreen: "Toggle fullscreen",
  ToggleFloatingPanes: "Show/hide floating panes",
  CloseFocus: "Close pane",
  CloseTab: "Close tab",
  ToggleActiveSyncTab: "Sync panes in tab",
  ToggleTab: "Circle tab focus",
  BreakPane: "Break pane to new tab",
  EditScrollback: "Open pane scrollback in editor",
  NewTab: "New tab",
  Detach: "Detach",
  Quit: "Quit",
};

// oxlint-disable-next-line eslint/max-lines-per-function -- This mirrors Wasm action grouping, including the fields lost at its protobuf boundary.
function actionType(action: Action): Pattern {
  if (typeof action !== "string") {
    if ("SwitchToMode" in action) return `Mode:${action.SwitchToMode.input_mode}`;
    if ("MovePane" in action)
      return action.MovePane.direction === null
        ? "MovePaneWithoutDirection"
        : "MovePaneWithDirection";
    if ("Resize" in action)
      return action.Resize.direction === null ? "ResizeAny" : `Resize${action.Resize.resize}`;
    if ("NewPane" in action)
      return action.NewPane.direction === null ? "NewPaneWithoutDirection" : "NewPaneWithDirection";
  }
  const name = tag(action);
  for (const [group, names] of Object.entries({
    BreakPaneLeftOrRight: ["BreakPaneLeft", "BreakPaneRight"],
    GoToAdjacentTab: ["GoToPreviousTab", "GoToNextTab"],
    Scroll: ["ScrollUp", "ScrollDown"],
    PageScroll: ["PageScrollUp", "PageScrollDown"],
    HalfPageScroll: ["HalfPageScrollUp", "HalfPageScrollDown"],
  }))
    if (names.includes(name)) return group;
  if (Object.hasOwn(descriptions, name)) return name;
  for (const [alias, group] of Object.entries({
    "session-manager": "SessionManager",
    configuration: "Configuration",
    "plugin-manager": "PluginManager",
    "yinz:about": "About",
  }))
    if (launches(alias)(action)) return group;
  if (typeof action !== "string") {
    const launch =
      "LaunchPlugin" in action
        ? action.LaunchPlugin
        : "LaunchOrFocusPlugin" in action
          ? action.LaunchOrFocusPlugin
          : null;
    if (launch && "Alias" in launch.plugin) {
      // The Wasm action wire retains launch options and the alias's own config,
      // but drops launch/alias cwd, resolved plugin metadata and focus options.
      return {
        OtherLaunch: {
          name,
          plugin: {
            name: launch.plugin.Alias.name,
            configuration: launch.plugin.Alias.configuration,
          },
          should_float: launch.should_float,
          should_open_in_place: launch.should_open_in_place,
          skip_cache: launch.skip_cache,
          move_to_focused_tab: "move_to_focused_tab" in launch ? launch.move_to_focused_tab : null,
        },
      };
    }
  }
  return { Other: action };
}

function description(type: Pattern): string {
  if (typeof type !== "string") return "Other action";
  if (type.startsWith("Mode:")) {
    const mode = type.slice(5);
    return (
      (
        {
          RenamePane: "Rename pane",
          RenameTab: "Rename tab",
          EnterSearch: "Search",
          Locked: "Lock",
          Normal: "Unlock",
        } as Record<string, string>
      )[mode] ?? mode
    );
  }
  return descriptions[type] ?? "Other action";
}

function groupedKeys(keys: readonly string[], brackets: boolean, locked: boolean): string {
  const nonEscape = keys.filter((key) => key !== "ESC" && key !== "ENTER");
  const filtered = locked && nonEscape.length > 0 ? nonEscape : keys;
  const wrap = (s: string): string => (brackets ? `<${s}>` : s);
  if (filtered.length === 1) return wrap(filtered[0] ?? "");
  const orders = [
    ["h", "j", "k", "l"],
    ["H", "J", "K", "L"],
    ["←", "↓", "↑", "→"],
    ["[", "]"],
    ["+", "-", "="],
    ["PgUp", "PgDn"],
  ];
  const groups: string[] = [];
  for (const [index, order] of orders.entries()) {
    let group = filtered
      .filter((key) => order.includes(key))
      .sort((a, b) => order.indexOf(a) - order.indexOf(b));
    if (index === 2) group = [...new Set(group)];
    if (index === 4 && group.includes("+")) group = group.filter((key) => key !== "=");
    if (group.length > 0) groups.push(wrap(group.join(index === 5 ? "|" : "")));
  }
  const other = filtered
    .filter((key) => !orders.some((order) => order.includes(key)))
    .map((key) => wrap(key));
  if (other.length > 0) groups.push(other.join("/"));
  return groups.join("/");
}

function actions(mi: ModeInfo): [string, string][] {
  const keymap = modeKeymap(mi),
    processed: Pattern[] = [],
    result: [string, string][] = [];
  for (const predicate of predicates(mi.mode)) {
    const first = keymap.find(([, aa]) => aa[0] !== undefined && predicate(aa[0]))?.[1][0];
    if (first === undefined) continue;
    const type = actionType(first);
    if (processed.some((previous) => equal(previous, type))) continue;
    const keys = keymap
      .filter(([, aa]) => aa[0] !== undefined && equal(actionType(aa[0]), type))
      .map(([key]) => keyLabel(key));
    result.push([
      groupedKeys(keys, mi.mode !== "Normal", type === "Mode:Locked"),
      description(type),
    ]);
    processed.push(type);
  }
  return result;
}

function contents(mi: ModeInfo): { actions: [string, string][]; rows: number; cols: number } {
  const entries = actions(mi);
  if (mi.mode === "Normal")
    return {
      actions: entries,
      rows: 1,
      cols: entries.reduce(
        (n, [key, label], i) => n + chars(key) + chars(label) + 5 + (i > 0 ? 1 : 0),
        0,
      ),
    };
  if (entries.length === 0) {
    const fallback =
      (
        {
          EnterSearch: "Entering search term...",
          RenameTab: "Renaming tab...",
          RenamePane: "Renaming pane...",
        } as Record<string, string>
      )[mi.mode] ?? mi.mode;
    return { actions: [[fallback, ""]], rows: 2, cols: chars(fallback) };
  }
  return {
    actions: entries,
    rows: entries.length + 1,
    cols:
      Math.max(...entries.map(([key]) => chars(key))) +
      Math.max(...entries.map(([, label]) => chars(label) + 2)) +
      1,
  };
}

export function dimensions(mi: ModeInfo): [number, number] {
  const { rows, cols } = contents(mi);
  return [rows + (mi.mode === "Normal" ? 0 : 1), cols];
}

// These components have unrestricted dimensions in the Wasm protocol. The SDK's
// viewport takes fixed dimensions, so retain its serialization and add only x/y.
function at(component: string, x: number, y: number, width?: number): string {
  return component.replace(";", `;${x}/${y}/${width ?? ""}/;`);
}

export function renderTooltip(mi: ModeInfo, rows: number, cols: number): string {
  const content = contents(mi);
  const x = Math.floor(Math.max(0, cols - content.cols) / 2),
    y = Math.floor(Math.max(0, rows - content.rows) / 2);
  if (mi.mode !== "Normal") {
    const table = z.ui.table().row(" ", " ");
    for (const [key, label] of content.actions) {
      if (label === "") {
        // Wasm emits a one-cell fallback row under a two-cell header. Keep its
        // wire shape; the SDK table builder correctly requires rectangular rows.
        const cell = z.ui.text(key).emphasis(0).render();
        const serialized =
          table.render().replace("table;2;1;", "table;2;2;").slice(0, -2) +
          ";" +
          cell.slice(cell.indexOf(";") + 1);
        return at(serialized, x, y) + "\u001B\\";
      }
      table.row(z.ui.text(key).emphasis(3), `- ${label}`);
    }
    return at(table.render(), x, y) + "\u001B\\";
  }
  let offset = 0,
    output = "";
  for (const [index, [key, label]] of content.actions.entries()) {
    const text = `${index === 0 ? "" : " "}${key} `;
    if (x + offset + chars(text) + chars(label) + 5 > cols) {
      output += at(
        z.ui.text("...").opaque().render(),
        x + offset,
        y,
        Math.max(0, cols - x - offset),
      );
      break;
    }
    output += at(z.ui.text(text).emphasis(3).opaque().render(), x + offset, y);
    output += at(z.ui.ribbon(label).render(), x + offset + chars(text), y);
    offset += chars(key) + chars(label) + 5 + (index === 0 ? 0 : 1);
  }
  return output;
}
