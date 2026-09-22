import z from "zellij:plugin";
import type { ModeInfo, PaletteColor, Styling } from "./keys.ts";
import {
  bareLabel,
  commonModifiers,
  keyLabel,
  keySeparator,
  modifiers,
  stripModifiers,
  type Key,
} from "./keys.ts";

export interface Line {
  part: string;
  len: number;
}
// oxlint-disable-next-line typescript/no-misused-spread -- The component protocol indexes Unicode scalars, matching Rust chars().
export const chars = (s: string): number => [...s].length;
export const width = (s: string): number => z.text.width(s);
// oxlint-disable-next-line eslint/no-control-regex -- Strip SGR escapes before measuring the rendered label.
export const plain = (s: string): string => s.replaceAll(/\u001B\[[0-9;]*m/gu, "");
export const line = (part = "", len = chars(plain(part))): Line => ({ part, len });
export const concat = (...parts: readonly Readonly<Line>[]): Line => ({
  part: parts.map((p) => p.part).join(""),
  len: parts.reduce((n, p) => n + p.len, 0),
});
// Some classic layout budgets historically count UTF-8 bytes. Keep those separate from
// terminal columns and the Unicode character indices used by component emphasis ranges.
export const byteLine = (part: string): Line => line(part, z.encodeUtf8(plain(part)).length);
export const color = (c: PaletteColor, bg = false): string =>
  "Rgb" in c ? `${bg ? 48 : 38};2;${c.Rgb.join(";")}` : `${bg ? 48 : 38};5;${c.EightBit}`;
export function paint(text: string, fg?: PaletteColor, bg?: PaletteColor, attributes = ""): string {
  if (!text) return "";
  const codes = [attributes, fg && color(fg), bg && color(bg, true)].filter(Boolean).join(";");
  return codes ? `\u001B[${codes}m${text}\u001B[0m` : text;
}
export const fill = (mi: ModeInfo): string =>
  `\u001B[${color(mi.style.colors.text_unselected.background, true)}m\u001B[0K`;
export const arrow = (mi: ModeInfo): string => (mi.capabilities.arrow_fonts ? "" : "");
export function textPart(text: string, emphasis?: number, dimmed = false): Line {
  const t = z.ui.text(text).opaque();
  if (dimmed) t.disabled();
  else if (emphasis !== undefined) t.emphasis(emphasis);
  return line(t.render(), width(text));
}

export function classicKeys(
  keys: readonly Key[],
  palette: Styling,
  background?: PaletteColor,
): string {
  if (keys.length === 0) return "";
  const common = commonModifiers(keys);
  const c = palette.text_unselected;
  let result =
    paint(common.join("-"), c.emphasis_0, background, "1") +
    paint(common.length > 0 ? " + <" : "<", c.base, background);
  const labels = keys.map((k) =>
    common.length > 0 ? keyLabel(stripModifiers(k, common)) : keyLabel(k),
  );
  const separator = paint(keySeparator(labels), c.base, background);
  result += labels.map((label) => paint(label, c.emphasis_2, background, "1")).join(separator);
  return result + paint(">", c.base, background);
}

export type TileMode = "unselected" | "alternate" | "selected" | "disabled";
export function tileColors(
  mi: ModeInfo,
  mode: TileMode,
  dimmed = false,
): Record<"prefix" | "key" | "bracket" | "text" | "suffix", (s: string) => string> {
  const p = mi.style.colors;
  const outer = p.text_unselected.background;
  const r = mode === "selected" && !dimmed ? p.ribbon_selected : p.ribbon_unselected;
  const bg = mode === "alternate" && mi.capabilities.arrow_fonts && !dimmed ? r.base : r.background;
  const fg = mode === "alternate" && !dimmed ? outer : r.base;
  const attributes = dimmed ? "3" : mode === "disabled" ? "2;3" : "1";
  return {
    prefix: (s: string): string => paint(s, outer, bg),
    key: (s: string): string =>
      paint(s, mode === "disabled" || dimmed ? r.base : r.emphasis_0, bg, attributes),
    bracket: (s: string): string => paint(s, fg, bg, attributes),
    text: (s: string): string => paint(s, r.base, bg, attributes),
    suffix: (s: string): string => paint(s, bg, outer, mode === "disabled" || dimmed ? "" : "1"),
  };
}
export const prefixPaint = (mi: ModeInfo, text: string, dimmed = false): string =>
  paint(
    text,
    mi.style.colors.text_unselected.base,
    mi.style.colors.text_unselected.background,
    dimmed ? "3" : "1",
  );
export const prefixSuffix = (mi: ModeInfo): string =>
  paint(
    arrow(mi),
    mi.style.colors.text_unselected.background,
    mi.style.colors.text_unselected.background,
  );
export const modifierPrefix = (mi: ModeInfo, mods: readonly string[]): string =>
  ` ${mods.join("-")} +${mi.capabilities.arrow_fonts ? " " : ""}`;
export function abbreviatedKey(key: Key, common: Key["key_modifiers"]): string {
  const k = stripModifiers(key, common);
  const abbreviations = { Ctrl: "^C", Alt: "^A", Super: "^Su", Shift: "^Sh" };
  return [
    modifiers
      .filter((m) => k.key_modifiers.includes(m))
      .map((m) => abbreviations[m])
      .join("-"),
    bareLabel(k.bare_key),
  ]
    .filter(Boolean)
    .join(" ");
}

function componentKeys(keys: readonly Key[], emphasis?: number, dimmed = false): Line {
  const common = commonModifiers(keys);
  const labels = keys.map((k) => keyLabel(keys.length > 1 ? stripModifiers(k, common) : k));
  const joined = labels.join(keySeparator(labels));
  const multi = common.length > 0 && keys.length > 1;
  const modifier = common.join("-");
  const content = multi ? ` ${modifier} <${joined}> ` : ` ${joined} `;
  const text = z.ui.text(content).opaque();
  if (dimmed) text.disabled();
  else if (emphasis !== undefined) {
    if (multi)
      text
        .emphasis(emphasis, 0, chars(modifier) + 1)
        .emphasis(emphasis, chars(modifier) + 3, chars(modifier) + 3 + chars(joined));
    else text.emphasis(emphasis);
  }
  return line(text.render(), width(content));
}

// oxlint-disable-next-line eslint/complexity -- The ribbon states mirror the shared text component styles and hover override.
export function shortcut(
  mi: ModeInfo,
  label: string,
  keys: readonly Key[],
  selected = false,
  emphasis = 3,
  inline = false,
  hovered = false,
  keyOnly = false,
): Line {
  if (keys.length === 0) return line();
  const dimmed = mi.session_dimmed ?? false;
  const labels = keys.map((k) => keyLabel(k));
  const joined = labels.join(keyOnly ? "-" : keySeparator(labels, true));
  const content = keyOnly ? joined : inline ? `<${joined}> ${label}` : label;
  let ribbon: string;
  if (hovered && !dimmed) {
    const p = mi.style.colors;
    const r = p.ribbon_unselected;
    const bg = r.emphasis_1;
    const body = inline
      ? paint(" <", r.base, bg, "1") +
        paint(joined, r.emphasis_0, bg, "1") +
        paint(`> ${label} `, r.base, bg, "1")
      : paint(` ${label} `, r.base, bg, "1");
    ribbon =
      paint(arrow(mi), p.text_unselected.background, bg) +
      body +
      paint(arrow(mi), bg, p.text_unselected.background);
  } else {
    const text = z.ui.ribbon(content);
    if (dimmed) text.disabled();
    else {
      if (selected) text.selected();
      if (keyOnly) text.emphasis(0);
      else if (inline) text.emphasis(0, 1, chars(joined) + 1);
    }
    ribbon = text.render();
  }
  const result = line(ribbon, width(content) + (mi.capabilities.arrow_fonts ? 2 : 4));
  return inline || keyOnly ? result : concat(componentKeys(keys, emphasis, dimmed), result);
}
