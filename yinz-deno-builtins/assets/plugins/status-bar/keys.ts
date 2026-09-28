import type * as Wire from "yinz:plugin";
import type { BareKey, InputMode, KeyModifier } from "yinz:plugin";

// Rendering reads host wire values without mutating their nested arrays or objects.
export type ReadonlyWire<T> = { readonly [K in keyof T]: ReadonlyWire<T[K]> };
export type ModeInfo = ReadonlyWire<Wire.ModeInfo>;
export type TabInfo = ReadonlyWire<Wire.TabInfo>;
export type Styling = ReadonlyWire<Wire.Styling>;
export type PaletteColor = ReadonlyWire<Wire.PaletteColor>;
export type Action = ReadonlyWire<Wire.Action>;
export type Key = ReadonlyWire<Wire.KeyWithModifier>;
export type Keymap = readonly (readonly [Key, readonly Action[]])[];
export type Pattern = string | Readonly<Record<string, unknown>>;
export const modifiers: KeyModifier[] = ["Ctrl", "Alt", "Shift", "Super"];
const bareOrder = [
  "PageDown",
  "PageUp",
  "Left",
  "Down",
  "Up",
  "Right",
  "Home",
  "End",
  "Backspace",
  "Delete",
  "Insert",
  "F",
  "Char",
  "Tab",
  "Esc",
  "Enter",
  "CapsLock",
  "ScrollLock",
  "NumLock",
  "PrintScreen",
  "Pause",
  "Menu",
];
export const tag = (value: Pattern): string =>
  typeof value === "string" ? value : (Object.keys(value)[0] ?? "");

// KeyWithModifier's equality treats an ASCII capital as Shift plus lowercase.
function canonicalKey(value: Readonly<Record<string, unknown>>): Readonly<Record<string, unknown>> {
  const bare = value["bare_key"];
  const mods = value["key_modifiers"];
  if (
    typeof bare === "object" &&
    bare !== null &&
    "Char" in bare &&
    typeof bare.Char === "string" &&
    /^[A-Z]$/u.test(bare.Char) &&
    Array.isArray(mods)
  ) {
    return {
      ...value,
      bare_key: { Char: bare.Char.toLowerCase() },
      key_modifiers: modifiers.filter((m) => m === "Shift" || mods.includes(m)),
    };
  }
  return value;
}

// Serde structs are compared by value, independently of their JSON field order.
export function equal(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return false;
  const left = canonicalKey(Object.fromEntries(Object.entries(a)));
  const right = canonicalKey(Object.fromEntries(Object.entries(b)));
  return (
    Object.keys(left).length === Object.keys(right).length &&
    Object.keys(left).every((k) => Object.hasOwn(right, k) && equal(left[k], right[k]))
  );
}
export function shallowEqual(a: Action, b: Pattern): boolean {
  return ["NewTab", "LaunchPlugin", "LaunchOrFocusPlugin", "OverrideLayout"].includes(tag(a))
    ? tag(a) === tag(b)
    : equal(a, b);
}
export const modeKeymap = (mi: ModeInfo, mode: InputMode = mi.mode): Keymap =>
  mi.keybinds.find(([m]) => m === mode)?.[1] ?? [];
export const switchMode = (mode: InputMode): Action => ({ SwitchToMode: { input_mode: mode } });
export const newPane = (direction: string | null = null): Pattern => ({
  NewPane: { direction, pane_name: null, start_suppressed: false },
});
export const resize = (size: string, direction: string | null = null): Pattern => ({
  Resize: { resize: size, direction },
});
export const directionAction = (name: string, direction: string | null): Pattern => ({
  [name]: { direction },
});
export const actionKeys = (km: Keymap, actions: readonly Pattern[]): Key[] =>
  km
    .filter(
      ([, aa]) =>
        aa.length === actions.length &&
        aa.every((a, i) => actions[i] !== undefined && shallowEqual(a, actions[i])),
    )
    .map(([k]) => k);
export const actionGroup = (km: Keymap, actions: readonly (readonly Pattern[])[]): Key[] =>
  actions.flatMap((a) => actionKeys(km, a));
export const commonModifiers = (keys: readonly Key[]): KeyModifier[] =>
  keys.length > 0 ? modifiers.filter((m) => keys.every((k) => k.key_modifiers.includes(m))) : [];
export const stripModifiers = (key: Key, common: readonly KeyModifier[]): Key => ({
  ...key,
  key_modifiers: key.key_modifiers.filter((m) => !common.includes(m)),
});
export const key = (
  bare_key: Readonly<BareKey>,
  key_modifiers: readonly KeyModifier[] = [],
): Key => ({ bare_key, key_modifiers });
export const charKey = (char: string, mods: readonly KeyModifier[] = []): Key =>
  key({ Char: char }, mods);
export function bareLabel(bare: Readonly<BareKey>): string {
  if (typeof bare === "object")
    return "Char" in bare ? (bare.Char === " " ? "SPACE" : bare.Char) : `F${bare.F}`;
  return (
    (
      {
        PageDown: "PgDn",
        PageUp: "PgUp",
        Left: "←",
        Down: "↓",
        Up: "↑",
        Right: "→",
        Delete: "DEL",
        Insert: "INS",
        CapsLock: "CAPSlOCK",
        ScrollLock: "SCROLLlOCK",
      } as Record<string, string>
    )[bare] ?? bare.toUpperCase()
  );
}
export const keyLabel = (k: Key): string =>
  [...modifiers.filter((m) => k.key_modifiers.includes(m)), bareLabel(k.bare_key)].join(" ");
export const genericKey = (k: Key): boolean =>
  ["ENTER", "ESC", "SPACE"].includes(bareLabel(k.bare_key));
export const toChar = (keys: readonly Key[]): Key | undefined =>
  keys.find((k) => !genericKey(k)) ?? keys[0];
const value = (k: Key): number =>
  typeof k.bare_key === "string"
    ? 0
    : "F" in k.bare_key
      ? k.bare_key.F
      : (k.bare_key.Char.codePointAt(0) ?? 0);
const rank = (k: Key): number => bareOrder.indexOf(tag(k.bare_key));
export function compareKeys(a: Key, b: Key): number {
  const diff = rank(a) - rank(b) || value(a) - value(b);
  if (diff) return diff;
  const am = modifiers.filter((m) => a.key_modifiers.includes(m));
  const bm = modifiers.filter((m) => b.key_modifiers.includes(m));
  for (let i = 0; i < Math.min(am.length, bm.length); i++) {
    const order = modifiers.indexOf(am[i] ?? "Ctrl") - modifiers.indexOf(bm[i] ?? "Ctrl");
    if (order) return order;
  }
  return am.length - bm.length;
}
export const ownKey = (keys: readonly Key[]): Key[] =>
  [...keys]
    .sort((a, b) => a.key_modifiers.length - b.key_modifiers.length || compareKeys(a, b))
    .slice(0, 1);
export function pluginKeys(km: Keymap, alias: string): Key[] {
  return ownKey(
    km
      .filter(([, actions]) =>
        actions.some((a) => {
          if (typeof a === "string") return false;
          const launch =
            "LaunchPlugin" in a
              ? a.LaunchPlugin
              : "LaunchOrFocusPlugin" in a
                ? a.LaunchOrFocusPlugin
                : undefined;
          if (!launch) return false;
          const p = launch.plugin;
          if ("Alias" in p) return p.Alias.name === alias;
          const location = p.RunPlugin.location;
          const url = "Yinz" in location ? `yinz:${location.Yinz}` : `file:${location.File}`;
          return url === alias;
        }),
      )
      .map(([k]) => k),
  );
}
// A MessagePlugin binding, by its message's name. The Wasm bar gets every KeybindPipe nameless
// (the plugin API's wire drops its fields) and so counts a nameless one; this keymap arrives
// whole, where the config always names a message, so the same rule only ever matches the name.
export const pipeKeys = (km: Keymap, name: string): Key[] =>
  ownKey(
    km
      .filter(([, actions]) =>
        actions.some(
          (a) =>
            typeof a !== "string" &&
            "KeybindPipe" in a &&
            (a.KeybindPipe.name === null || a.KeybindPipe.name === name),
        ),
      )
      .map(([k]) => k),
  );
export const keySeparator = (labels: readonly string[], inline = false): string =>
  ["HJKL", "hjkl", "←↓↑→", "←→", "↓↑", "[]", ...(inline ? ["+-"] : [])].includes(labels.join(""))
    ? ""
    : "|";
