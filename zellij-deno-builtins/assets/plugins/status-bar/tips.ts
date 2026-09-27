import z from "zellij:plugin";
import type { ModeInfo } from "./keys.ts";
import {
  actionGroup,
  actionKeys,
  charKey,
  directionAction as dir,
  equal,
  keyLabel,
  modeKeymap,
  newPane,
  resize,
  switchMode,
  type Key,
  type Pattern,
} from "./keys.ts";
import { byteLine, classicKeys, line, paint, type Line } from "./style.ts";

const tipNames = [
  "quicknav",
  "floating_panes_mouse",
  "send_mouse_clicks_to_terminal",
  "move_focus_hjkl_tab_switch",
  "zellij_setup_check",
  "use_mouse",
  "sync_tab",
  "edit_scrollbuffer",
  "compact_layout",
  "move_tabs",
];
const cachePath = "/tmp/status-bar-tips.cache";
interface Cache {
  zellij_version: string;
  cached_data: Record<string, number>;
}
function isCache(value: unknown): value is Cache {
  if (
    typeof value !== "object" ||
    value === null ||
    !("zellij_version" in value) ||
    typeof value.zellij_version !== "string" ||
    !("cached_data" in value)
  )
    return false;
  const counts = value.cached_data;
  return (
    typeof counts === "object" &&
    counts !== null &&
    !Array.isArray(counts) &&
    Object.values(counts).every(
      (n: unknown) => typeof n === "number" && Number.isSafeInteger(n) && n >= 0,
    )
  );
}
export function cachedTip(): string {
  try {
    const source = z.fs.metadata(cachePath) ? z.decodeUtf8(z.fs.read(cachePath)) : "";
    const version = z.command("GetZellijVersion");
    const parsed: unknown = source
      ? JSON.parse(source)
      : { zellij_version: version, cached_data: {} };
    if (!isCache(parsed)) return "quicknav";
    const cache = parsed;
    if (cache.zellij_version !== version) {
      cache.zellij_version = version;
      cache.cached_data = {};
    }
    if (Object.keys(cache.cached_data).length === 0)
      cache.cached_data = Object.fromEntries(tipNames.map((name) => [name, 1]));
    const names = Object.keys(cache.cached_data);
    const chosen =
      (cache.cached_data["quicknav"] ?? 0) <= 20
        ? "quicknav"
        : (names[Math.floor(Math.random() * names.length)] ?? "quicknav");
    cache.cached_data[chosen] = (cache.cached_data[chosen] ?? 0) + 1;
    z.fs.write(cachePath, JSON.stringify(cache));
    return chosen;
  } catch {
    return "quicknav";
  }
}

// oxlint-disable-next-line eslint/max-lines-per-function -- Preserve the full, medium and short forms of the reference tip table together.
export function tip(mi: ModeInfo, name: string, max: number): Line {
  const p = mi.style.colors;
  const emph = (text: string, n: 0 | 2 = 0): string =>
    paint(
      text,
      n === 0 ? p.text_unselected.emphasis_0 : p.text_unselected.emphasis_2,
      undefined,
      "1",
    );
  const unbound = paint("UNBOUND", undefined, undefined, "1");
  const keys = (kk: readonly Key[]): string => (kk.length > 0 ? classicKeys(kk, p) : unbound);
  const km = modeKeymap(mi);
  const sequence = (mode: "Pane" | "Tab" | "Scroll", action: Pattern): string => {
    const kk = actionKeys(modeKeymap(mi, mode), [action, switchMode("Normal")]);
    return kk.length > 0
      ? classicKeys(actionKeys(km, [switchMode(mode)]), p) + ", " + classicKeys(kk, p)
      : unbound;
  };
  const navigation = (kk: readonly Key[]): string => {
    const arrows = kk.filter((k) => /[←↓↑→]/u.test(keyLabel(k)));
    const letters = kk.filter((k) => !/[←↓↑→]/u.test(keyLabel(k)));
    return kk.length === 0
      ? unbound
      : [arrows, letters]
          .filter((k) => k.length > 0)
          .map((k) => classicKeys(k, p))
          .join(" or ");
  };
  let forms: string[];
  switch (name) {
    case "floating_panes_mouse": {
      const k = sequence("Pane", "ToggleFloatingPanes");
      forms = [
        ` Tip: Toggle floating panes with ${k} and move them with keyboard or mouse`,
        ` Tip: Toggle floating panes with ${k}`,
        `${k} => floating panes`,
      ];
      break;
    }
    case "send_mouse_clicks_to_terminal": {
      const k = ` Tip: ${emph("Shift")} + <${emph("mouse-click", 2)}>`;
      forms = [
        k + " bypasses Zellij and sends the mouse click directly to the terminal.",
        k + " sends the click directly to the terminal.",
        k + " => sends click to terminal.",
      ];
      break;
    }
    case "move_focus_hjkl_tab_switch": {
      const k = navigation(
        actionGroup(modeKeymap(mi, "Pane"), [
          [dir("MoveFocusOrTab", "Left")],
          [dir("MoveFocusOrTab", "Right")],
        ]),
      );
      forms = [
        ` Tip: When changing focus with ${k} moving off screen left/right focuses the next tab.`,
        ` Tip: Changing focus with ${k} off screen focuses the next tab.`,
        `${k} off screen edge focuses next tab.`,
      ];
      break;
    }
    case "zellij_setup_check": {
      const command = emph("zellij setup --check");
      forms = [
        ` Tip: Having issues with Zellij? Try running ${command}`,
        ` Tip: Run ${command} to find issues`,
        ` Run ${command} to find issues`,
      ];
      break;
    }
    case "use_mouse":
      forms = [
        ` Tip: ${emph("Use the mouse", 2)} to switch pane focus, scroll through the pane scrollbuffer, switch or scroll through the tabs.`,
        ` Tip: ${emph("Use the mouse", 2)} to switch pane/tabs or scroll through the pane scrollbuffer.`,
        `${emph(" Use the mouse", 2)} to switch pane/tabs or scroll.`,
      ];
      break;
    case "sync_tab": {
      const k = sequence("Tab", "ToggleActiveSyncTab");
      forms = [
        ` Tip: Sync a tab and write keyboard input to all its panes with ${k}`,
        ` Tip: Sync input to panes in a tab with ${k}`,
        ` Sync input in a tab with ${k}`,
      ];
      break;
    }
    case "edit_scrollbuffer": {
      const k = sequence("Scroll", { EditScrollback: { ansi: false } });
      const editor = emph("$EDITOR");
      forms = [
        ` Tip: Search through the scrollbuffer using your default ${editor} with ${k}`,
        ` Tip: Search the scrollbuffer using your ${editor} with ${k}`,
        ` Search using ${editor} with ${k}`,
      ];
      break;
    }
    case "compact_layout": {
      const k = sequence("Pane", "TogglePaneFrames");
      const command = emph("zellij -l compact");
      forms = [
        ` Tip: UI taking up too much space? Start Zellij with ${command} or remove pane frames with ${k}`,
        ` Tip: To save screen space, start Zellij with ${command} or remove frames with ${k}`,
        ` Save screen space, start with: ${command} or remove frames with ${k}`,
      ];
      break;
    }
    case "move_tabs": {
      const k = `${emph("Alt + i", 2)} (left) and ${emph("Alt + o", 2)} (right)`;
      forms = [
        ` Tip: Wrong order of tabs? You can move them to left and right with: ${k}`,
        ` Tip: You can move tabs to left and right with: ${k}`,
        ` Move tabs with: ${k}`,
      ];
      break;
    }
    default: {
      const pane = keys(actionKeys(km, [newPane()]));
      let rr = actionGroup(km, [[resize("Increase")], [resize("Decrease")]]);
      if (
        rr.some((k) => equal(k, charKey("=", ["Alt"]))) &&
        rr.some((k) => equal(k, charKey("+", ["Alt"])))
      )
        rr = rr.filter((k) => !equal(k, charKey("=", ["Alt"])));
      const size = keys(rr);
      const focus = navigation(
        actionGroup(km, [
          [dir("MoveFocus", "Left")],
          [dir("MoveFocusOrTab", "Left")],
          [dir("MoveFocus", "Down")],
          [dir("MoveFocus", "Up")],
          [dir("MoveFocus", "Right")],
          [dir("MoveFocusOrTab", "Right")],
        ]),
      );
      forms = [
        ` Tip: ${pane} => open new pane. ${focus} => navigate between panes. ${size} => increase/decrease pane size.`,
        ` Tip: ${pane} => new pane. ${focus} => navigate. ${size} => resize pane.`,
        ` QuickNav: ${pane} / ${focus} / ${size}`,
      ];
    }
  }
  return forms.map((part) => byteLine(part)).find((l) => l.len <= max) ?? line();
}
