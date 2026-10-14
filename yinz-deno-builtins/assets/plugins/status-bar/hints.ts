import type { ModeInfo } from "./keys.ts";
import {
  actionGroup,
  actionKeys,
  compareKeys,
  directionAction as dir,
  equal,
  key,
  modeKeymap,
  newPane,
  ownKey,
  pipeKeys,
  pluginKeys,
  resize,
  shallowEqual,
  switchMode,
  type Key,
  type Pattern,
} from "./keys.ts";
import { concat, line, type Line } from "./style.ts";

interface Hint {
  long: string;
  short: string;
  keys: readonly Key[];
}
const hint = (long: string, short: string, keys: readonly Key[]): Hint => ({ long, short, keys });
const searchInput = (input: number): Pattern => ({ SearchInput: { input: [input] } });

// oxlint-disable-next-line eslint/max-lines-per-function -- Keep the mode hint table together; both layouts use the same selection rules.
export function hintsForMode(mi: ModeInfo): Hint[] {
  const original = modeKeymap(mi);
  const exits = actionKeys(original, [switchMode(mi.base_mode ?? "Normal")]);
  const back = (preferred: "Enter" | "Esc"): Key[] =>
    exits.some((k) => equal(k, key(preferred))) ? [key(preferred)] : exits.slice(0, 1);
  const toBase = back("Enter");
  const escape = back("Esc");
  const copy = ownKey(
    original.filter(([, aa]) => aa[0] !== undefined && shallowEqual(aa[0], "Copy")).map(([k]) => k),
  );
  // Tab focus reads best as ←→, but arrows sort Left, Down, Up, Right and the dedupe below drops
  // Right for Down: name the arrow pointing each way when it does it, from the whole keymap.
  const preferring = (a: Pattern, preferred: "Left" | "Right"): Key[] => {
    const keys = original
      .filter(([, aa]) => aa[0] !== undefined && shallowEqual(aa[0], a))
      .map(([k]) => k);
    return keys.some((k) => equal(k, key(preferred))) ? [key(preferred)] : ownKey(keys);
  };
  const known: unknown[] = [];
  const km = [...original]
    .sort(([a], [b]) => compareKeys(a, b))
    .filter(([, aa]) => {
      if (known.some((a) => equal(a, aa))) return false;
      known.push(aa);
      return true;
    });
  const doing = (a: Pattern): Key[] =>
    ownKey(km.filter(([, aa]) => aa[0] !== undefined && shallowEqual(aa[0], a)).map(([k]) => k));
  const each = (aa: readonly Pattern[]): Key[] => aa.flatMap((a) => doing(a));
  const exact = (...aa: readonly Pattern[]): Key[] => actionKeys(km, aa);
  const group = (...aa: readonly Pattern[]): Key[] =>
    actionGroup(
      km,
      aa.map((a) => [a]),
    );
  const plugin = (alias: string): Key[] => pluginKeys(km, alias);
  const focus = ["Left", "Down", "Up", "Right"].map((d) => dir("MoveFocus", d));
  const select = hint("Select pane", "Select", toBase);
  switch (mi.mode) {
    case "Pane":
      return [
        hint("New", "New", doing(newPane())),
        hint("Change Focus", "Move", each(focus)),
        hint("Close", "Close", doing("CloseFocus")),
        hint("Rename", "Rename", doing(switchMode("RenamePane"))),
        hint("Toggle Fullscreen", "Fullscreen", doing("ToggleFocusFullscreen")),
        hint("Toggle Floating", "Floating", doing("ToggleFloatingPanes")),
        hint("Toggle Embed", "Embed", doing("TogglePaneEmbedOrFloating")),
        hint("Split Right", "Right", doing(newPane("Right"))),
        hint("Split Down", "Down", doing(newPane("Down"))),
        hint(
          "Stack",
          "Stack",
          doing({
            NewStackedPane: {
              command: null,
              pane_name: null,
              near_current_pane: false,
              no_focus: false,
              tab_id: null,
            },
          }),
        ),
        hint("Select text", "Text", doing(switchMode("Select"))),
        select,
      ];
    case "Tab":
      return [
        hint("New", "New", doing({ NewTab: {} })),
        hint("Change focus", "Move", [
          ...preferring("GoToPreviousTab", "Left"),
          ...preferring("GoToNextTab", "Right"),
        ]),
        hint("Close", "Close", doing("CloseTab")),
        hint("Rename", "Rename", doing(switchMode("RenameTab"))),
        hint("Sync", "Sync", doing("ToggleActiveSyncTab")),
        hint("Break pane to new tab", "Break out", doing("BreakPane")),
        hint("Break pane left/right", "Break", each(["BreakPaneLeft", "BreakPaneRight"])),
        hint("Toggle", "Toggle", doing("ToggleTab")),
        select,
      ];
    case "Resize":
      return [
        hint(
          "Increase/Decrease size",
          "Increase/Decrease",
          group(resize("Increase"), resize("Decrease")),
        ),
        hint(
          "Increase to",
          "Increase",
          group(...["Left", "Down", "Up", "Right"].map((d) => resize("Increase", d))),
        ),
        hint(
          "Decrease from",
          "Decrease",
          group(...["Left", "Down", "Up", "Right"].map((d) => resize("Decrease", d))),
        ),
        select,
      ];
    case "Move":
      return [
        hint(
          "Switch Location",
          "Move",
          group(...["Left", "Down", "Up", "Right"].map((d) => dir("MovePane", d))),
        ),
        hint("When done", "Back", toBase),
      ];
    case "Scroll":
      return [
        hint("Enter search term", "Search", exact(switchMode("EnterSearch"), searchInput(0))),
        hint(
          "Scroll",
          "Scroll",
          group(
            "ScrollDown",
            "ScrollUp",
            "PageScrollDown",
            "PageScrollUp",
            "HalfPageScrollDown",
            "HalfPageScrollUp",
          ),
        ),
        hint(
          "Edit scrollback in default editor",
          "Edit",
          doing({ EditScrollback: { ansi: false } }),
        ),
        select,
        hint("Scroll commands", "Commands", group("ScrollToPreviousPrompt", "ScrollToNextPrompt")),
      ];
    case "Select":
      return [
        hint(
          "Move selection",
          "Move",
          group(...["Left", "Down", "Up", "Right"].map((d) => dir("MoveTextSelection", d))),
        ),
        hint("Swap ends", "Swap", doing("SwapTextSelectionEnds")),
        hint("Copy and exit", "Copy", copy.length > 0 ? copy : toBase),
      ];
    case "EnterSearch":
      return [
        hint("When done", "Done", exact(switchMode("Search"))),
        hint("Cancel", "Cancel", exact(searchInput(27), switchMode("Scroll"))),
      ];
    case "Search":
      return [
        hint("Enter Search term", "Search", exact(switchMode("EnterSearch"), searchInput(0))),
        hint("Scroll", "Scroll", group("ScrollDown", "ScrollUp")),
        hint("Scroll page", "Scroll", group("PageScrollDown", "PageScrollUp")),
        hint("Scroll half page", "Scroll", group("HalfPageScrollDown", "HalfPageScrollUp")),
        hint("Search down", "Down", exact(dir("Search", "Down"))),
        hint("Search up", "Up", exact(dir("Search", "Up"))),
        hint(
          "Case sensitive",
          "Case",
          exact({ SearchToggleOption: { option: "CaseSensitivity" } }),
        ),
        hint("Wrap", "Wrap", exact({ SearchToggleOption: { option: "Wrap" } })),
        hint("Whole words", "Whole", exact({ SearchToggleOption: { option: "WholeWord" } })),
      ];
    case "Session":
      return [
        hint("Detach", "Detach", exact("Detach")),
        hint("Session Manager", "Manager", plugin("session-manager")),
        hint("Configure", "Config", plugin("configuration")),
        hint("Layout Manager", "Layouts", plugin("yinz:layout-manager")),
        hint("Plugin Manager", "Plugins", plugin("plugin-manager")),
        hint("Notifications", "Notif", exact(switchMode("Notifications"))),
        select,
      ];
    case "Notifications":
      return [
        hint(
          "Select",
          "Select",
          group(dir("MoveNotificationSelection", "Down"), dir("MoveNotificationSelection", "Up")),
        ),
        hint("Open", "Open", doing("ActivateNotification")),
        hint("Full text", "Full", doing("ToggleNotificationDetail")),
        hint("Exit", "Exit", escape),
      ];
    case "Agent":
      return [
        hint("New agent", "New", plugin("new-agent")),
        hint("Duplicate agent", "Duplicate", plugin("duplicate-agent")),
        hint("Fork agent", "Fork", plugin("fork-agent")),
        hint("Agents", "Agents", plugin("agents")),
        hint("Move panel", "Move", pipeKeys(km, "agent_strip_reposition")),
        select,
      ];
    case "Tmux":
      return [
        hint("Move focus", "Move", each(focus)),
        hint("Split down", "Down", doing(newPane("Down"))),
        hint("Split right", "Right", doing(newPane("Right"))),
        hint("Fullscreen", "Fullscreen", doing("ToggleFocusFullscreen")),
        hint("New tab", "New", doing({ NewTab: {} })),
        hint("Rename tab", "Rename", doing(switchMode("RenameTab"))),
        hint("Previous Tab", "Previous", doing("GoToPreviousTab")),
        hint("Next Tab", "Next", doing("GoToNextTab")),
        select,
      ];
    case "RenamePane":
    case "RenameTab":
      return [hint("When done", "Done", toBase), hint("Select pane", "Select", each(focus))];
    case "Normal":
    case "Locked":
    case "Prompt":
      return [];
    default:
      throw new Error(`Unknown input mode: ${String(mi.mode)}`);
  }
}

export function renderHints(
  mi: ModeInfo,
  max: number,
  render: (first: boolean, label: string, keys: readonly Key[]) => Line,
): Line {
  const hints = hintsForMode(mi);
  for (const label of ["long", "short"] as const) {
    let result = line();
    for (const h of hints) result = concat(result, render(result.len === 0, h[label], h.keys));
    if (result.len <= max) return result;
  }
  let result = line();
  for (const h of hints) {
    const next = render(result.len === 0, h.short, h.keys);
    if (result.len + next.len + 5 > max) {
      if (result.len + 5 <= max) result = concat(result, line(" ... "));
      break;
    }
    result = concat(result, next);
  }
  return result;
}
