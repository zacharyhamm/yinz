import type { ReadonlyWire } from "./status-bar/keys.ts";
import yinz, {
  definePlugin,
  type ConfigurationNode as Node,
  type ConfigurationSnapshot as SnapshotWire,
  type ConfigurationPatch as Patch,
  type ConfigurationSetting as SettingWire,
  type PaletteAction,
  type PaletteField,
  type PaletteView,
  type PaletteCompletion,
} from "yinz:plugin";

type Snapshot = ReadonlyWire<SnapshotWire>;
type Setting = ReadonlyWire<SettingWire>;
type ReadNode = ReadonlyWire<Node>;
type Scalar = string | number | boolean;
const clean = (value: string): string =>
  yinz.text.sanitize(value).replaceAll("\n", " ").replaceAll("\t", " ");
function clone(value: ReadNode): Node;
function clone(value: undefined): undefined;
function clone(value: ReadNode | undefined): Node | undefined;
function clone(value: ReadNode | undefined): Node | undefined {
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- Configuration nodes contain only JSON primitives, so copying preserves the schema.
  return value === undefined ? undefined : (JSON.parse(JSON.stringify(value)) as Node);
}
const equal = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);
const nodeKey = (node: ReadNode): string =>
  ["bind", "unbind"].includes(node.name)
    ? `${node.name}:${JSON.stringify(node.arguments)}`
    : node.name;
const node = (
  name: string,
  args: readonly Scalar[] = [],
  children: readonly ReadNode[] = [],
): Node => ({
  name,
  arguments: [...args],
  properties: {},
  children: children.map((child) => clone(child)),
});
const read = (theme: string | null = null): Snapshot => yinz.command("InspectConfiguration", theme);
function find(nodes: readonly ReadNode[], path: readonly string[]): ReadNode | undefined {
  const [name, ...rest] = path;
  const found = nodes.find((n) => nodeKey(n) === name);
  return rest.length > 0 && found ? find(found.children, rest) : found;
}
function list(title: string, actions: () => PaletteAction[]): PaletteView {
  return {
    type: "actions",
    title,
    actions: (request) => {
      const words = request.query.toLowerCase().split(/\s+/u).filter(Boolean);
      return actions()
        .filter((a) =>
          words.every((w) => `${a.label} ${(a.aliases ?? []).join(" ")}`.toLowerCase().includes(w)),
        )
        .slice(0, 100);
    },
  };
}
function confirm(title: string, run: () => PaletteView | PaletteCompletion): PaletteView {
  return {
    type: "form",
    title,
    fields: [],
    confirmation: true,
    submitLabel: "Confirm",
    onSubmit: run,
  };
}
function scalarField(
  id: string,
  label: string,
  value: Scalar,
  kind?: string,
  choices?: readonly string[],
  help = "",
): PaletteField {
  if (typeof value === "boolean")
    return {
      type: "choice",
      id,
      label,
      options: ["Enabled", "Disabled"],
      initialValue: value ? "Enabled" : "Disabled",
    };
  if (choices?.length)
    return {
      type: "choice",
      id,
      label,
      options:
        !String(value) || choices.includes(String(value))
          ? [...choices]
          : [String(value), ...choices],
      initialValue: String(value) === "" ? (choices[0] ?? "") : String(value),
    };
  const type =
    kind === "number" || typeof value === "number"
      ? "number"
      : kind === "file" || kind === "directory" || kind === "key"
        ? kind
        : "multiline";
  return {
    type,
    id,
    label,
    initialValue: String(value),
    help:
      help ||
      (type === "key"
        ? "Type a chord, or press F2 then the desired key. Tab moves to the next field."
        : type === "multiline"
          ? "Shift+Enter inserts a newline. Whitespace is preserved."
          : ""),
  };
}
function parseScalar(text: string, old: Scalar): Scalar {
  if (typeof old === "boolean") return text === "Enabled";
  if (typeof old === "number") {
    const number = Number(text);
    if (!/^-?\d+$/u.test(text) || !Number.isSafeInteger(number))
      throw new Error("Enter a whole number in range");
    return number;
  }
  return text;
}
function diff(
  before: ReadNode | undefined,
  after: ReadNode | undefined,
  path: readonly string[],
): Patch[] {
  if (equal(before, after)) return [];
  if (
    !before ||
    !after ||
    !equal(before.arguments, after.arguments) ||
    !equal(before.properties, after.properties) ||
    path[0] === "context_menu" ||
    before.name === "bind" ||
    new Set(before.children.map((item) => nodeKey(item))).size !== before.children.length ||
    new Set(after.children.map((item) => nodeKey(item))).size !== after.children.length
  )
    return [{ path: [...path], node: clone(after) ?? null }];
  return [...new Set([...before.children, ...after.children].map((item) => nodeKey(item)))].flatMap(
    (name) =>
      diff(
        before.children.find((n) => nodeKey(n) === name),
        after.children.find((n) => nodeKey(n) === name),
        [...path, name],
      ),
  );
}
class Draft {
  public readonly original: Node | undefined;
  public value: Node;
  public constructor(
    public snapshot: Snapshot,
    public readonly client: number,
    public readonly root: string,
  ) {
    this.original = clone(find(snapshot.clients[String(client)] ?? snapshot.saved, [root]));
    this.value = clone(this.original ?? node(root));
  }
  // oxlint-disable-next-line eslint/max-lines-per-function -- Keep destination and reload callbacks with their captured patches.
  public commit(
    patches: readonly ReadonlyWire<Patch>[],
    reset: readonly (readonly string[])[] = [],
    permanentPatches: readonly ReadonlyWire<Patch>[] = patches,
  ): PaletteView {
    const setting = this.snapshot.catalog.find((s) => s.name === this.root);
    const timing =
      setting?.timing ?? this.snapshot.section_timing[this.root] ?? "Applies to this session";
    const startup = timing === "On next session start";
    return {
      type: "form",
      title: `${clean(this.root)} — ${timing}`,
      fields: [
        {
          type: "choice",
          id: "destination",
          label: "Destination",
          options: startup ? ["Save permanently"] : ["Apply to session", "Save permanently"],
          initialValue: startup ? "Save permanently" : "Apply to session",
        },
      ],
      submitLabel: "Apply change",
      actions: [
        {
          id: "reload",
          label: "Reload configuration (keep draft)",
          run: () => {
            const fresh = read();
            return confirm(
              "Configuration reloaded. Reapply this draft to the current configuration?",
              () =>
                new Draft(fresh, this.client, this.root).commit(patches, reset, permanentPatches),
            );
          },
        },
      ],
      onSubmit: (values) => {
        const result = yinz.command("EditConfiguration", {
          revision: this.snapshot.revision,
          destination: values["destination"] === "Save permanently" ? "permanent" : "session",
          patches: (values["destination"] === "Save permanently" ? permanentPatches : patches).map(
            (patch) => ({
              path: [...patch.path],
              node: clone(patch.node ?? undefined) ?? null,
            }),
          ),
          reset: reset.map((path) => [...path]),
        });
        return { type: "complete", notification: result.notice };
      },
    };
  }
  public hydrateTheme(name: string): void {
    const fresh = read(name);
    if (fresh.revision !== this.snapshot.revision)
      throw new Error("Configuration changed. Reopen this editor before loading another theme.");
    const loaded = find(fresh.clients[String(this.client)] ?? fresh.saved, ["themes", name]);
    if (!loaded) throw new Error("Theme is no longer available");
    const current = this.value.children.find((child) => child.name === name);
    const original = this.original?.children.find((child) => child.name === name);
    if (current) Object.assign(current, clone(loaded));
    if (original) Object.assign(original, clone(loaded));
    this.snapshot = fresh;
  }
  public finish(): PaletteView {
    const patches = diff(this.original, this.value, [this.root]);
    return this.commit(patches);
  }
}
function stateHelp(draft: ReadonlyWire<Draft>, path: readonly string[]): string {
  const saved = find(draft.snapshot.saved, path);
  const clients = Object.entries(draft.snapshot.clients).map(
    ([id, nodes]) => `Client ${id}: ${JSON.stringify(find(nodes, path)?.arguments ?? "unset")}`,
  );
  const override = draft.snapshot.overrides.some(
    (p) =>
      path.every((part, i) => p.path[i] === part) || p.path.every((part, i) => path[i] === part),
  );
  return `Saved: ${JSON.stringify(saved?.arguments ?? "unset")}\n${clients.join("\n")}\nSession override: ${override ? "Yes" : "No"}`;
}
// oxlint-disable-next-line eslint/max-lines-per-function, typescript/prefer-readonly-parameter-types -- Keep the form with its callbacks; Submission changes the shared draft.
function settingForm(draft: Draft, setting: Setting): PaletteView {
  let value =
    draft.value.arguments[0] ??
    (setting.kind === "boolean" ? false : setting.kind === "number" ? (setting.minimum ?? 0) : "");
  if (setting.kind === "number" && typeof value === "string")
    value = Number(value.replace(/%$/u, ""));
  const choices =
    setting.kind === "theme"
      ? (find(draft.snapshot.saved, ["themes"])?.children.map((n) => n.name) ?? [])
      : setting.choices;
  const field = scalarField(
    "value",
    setting.label,
    value,
    setting.kind,
    setting.kind === "boolean" ? undefined : [...choices],
    `${setting.help}\n${stateHelp(draft, [setting.name])}`,
  );
  if (field.type === "number") {
    if (setting.minimum !== null) field.minimum = setting.minimum;
    if (setting.maximum !== null) field.maximum = setting.maximum;
  }
  return {
    type: "form",
    title: `${setting.label} — ${setting.timing}. ${stateHelp(draft, [setting.name]).replaceAll("\n", "; ")}`,
    fields: [field],
    submitLabel: "Choose destination…",
    onSubmit: (values) => {
      draft.value.arguments = [parseScalar(values["value"] ?? "", value)];
      const effectivePatches = diff(draft.original, draft.value, [draft.root]);
      const savedPatches = diff(find(draft.snapshot.saved, [draft.root]), draft.value, [
        draft.root,
      ]);
      return draft.commit(
        effectivePatches,
        [],
        savedPatches.length > 0 ? savedPatches : effectivePatches,
      );
    },
    actions: [
      {
        id: "state",
        label: "Current values and effect timing…",
        run: () => ({
          type: "text",
          title: setting.label,
          text: `${setting.help}\n${setting.timing}\n${stateHelp(draft, [setting.name])}`,
        }),
      },
      {
        id: "unset",
        label: "Remove configured value…",
        run: () =>
          confirm(
            "Remove this configured value? The setting's automatic behavior will apply.",
            () => draft.commit([{ path: [setting.name], node: null }]),
          ),
      },
      { id: "reset", label: "Reset override…", run: () => draft.commit([], [[setting.name]]) },
    ],
  };
}
function templates(
  draft: ReadonlyWire<Draft>,
  path: readonly string[],
  parent: ReadNode,
): readonly ReadNode[] {
  const children = draft.snapshot.action_children[parent.name];
  if (children) return children;
  const match = Object.entries(draft.snapshot.templates).find(([pattern]) => {
    const parts = pattern.split("/");
    return (
      parts.length === path.length && parts.every((part, i) => part === "*" || part === path[i])
    );
  });
  return match?.[1] ?? [node("entry", [""])];
}
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Child actions mutate their shared parent and draft.
function childList(draft: Draft, parent: Node, path: readonly string[]): PaletteView {
  return list(`${clean(path.join(" / "))} — draft`, () => [
    { id: "save", label: "Review and apply draft…", run: () => draft.finish() },
    {
      id: "add",
      label: "Add entry…",
      run: () =>
        list("Choose entry", () =>
          templates(draft, path, parent).map((template, index) => ({
            id: `template:${index}`,
            label: template.name,
            run: () => {
              const child = clone(template);
              return nodeForm(draft, child, [...path, nodeKey(child)], () => {
                const conflicts = parent.children.filter(
                  (n) => n !== child && nodeKey(n) === nodeKey(child),
                );
                const append = (): PaletteView => {
                  const ordered = path[0] === "context_menu" || parent.name === "bind";
                  if (!ordered)
                    parent.children = parent.children.filter((n) => nodeKey(n) !== nodeKey(child));
                  parent.children.push(child);
                  return childList(draft, parent, path);
                };
                return conflicts.length > 0 && path[0] !== "context_menu" && parent.name !== "bind"
                  ? confirm(
                      `Replace ${clean(nodeKey(child))}? Existing actions: ${conflicts.flatMap((n) => n.children.map((a) => a.name)).join(", ")}`,
                      append,
                    )
                  : append();
              });
            },
          })),
        ),
    },
    { id: "reset", label: "Reset override…", run: () => draft.commit([], [path]) },
    ...parent.children.map((child, index): PaletteAction => ({
      id: `entry:${index}`,
      label: clean(
        `${index + 1}. ${child.name}${child.name === "bind" ? ` ${child.arguments.join(" ")}` : ""}`,
      ),
      run: () => entry(draft, parent, index, path),
    })),
  ]);
}
// oxlint-disable-next-line eslint/max-lines-per-function, typescript/prefer-readonly-parameter-types -- Keep the form with its callbacks; Entry actions mutate their shared parent and draft.
function entry(draft: Draft, parent: Node, index: number, path: readonly string[]): PaletteView {
  const child = parent.children[index];
  if (!child) throw new Error("Entry was removed from the draft");
  if (path.length === 1 && path[0] === "themes" && child.children.length === 0)
    draft.hydrateTheme(child.name);
  const childPath = [...path, nodeKey(child)];
  // oxlint-disable-next-line eslint/max-lines-per-function -- Entry actions share one draft node and preserve navigation order.
  return list(clean(childPath.join(" / ")), () => [
    {
      id: "state",
      label: "Current values…",
      run: () => ({
        type: "text",
        title: clean(childPath.join(" / ")),
        text: stateHelp(draft, childPath),
      }),
    },
    {
      id: "edit",
      label: "Edit fields…",
      run: () =>
        nodeForm(draft, child, childPath, () =>
          entry(draft, parent, parent.children.indexOf(child), path),
        ),
    },
    {
      id: "children",
      label: "Edit nested entries / actions…",
      run: () => childList(draft, child, childPath),
    },
    {
      id: "remove",
      label: "Remove entry…",
      run: () =>
        confirm(`Remove ${clean(child.name)} from this draft?`, () => {
          parent.children.splice(index, 1);
          return childList(draft, parent, path);
        }),
    },
    ...(path[0] === "hooks"
      ? [
          {
            id: "disable",
            label: "Disable hook",
            run: () => {
              child.arguments = [""];
              child.children = [];
              return childList(draft, parent, path);
            },
          },
        ]
      : []),
    {
      id: "up",
      label: "Move earlier",
      run: () => {
        if (index > 0) {
          parent.children.splice(index, 1);
          parent.children.splice(index - 1, 0, child);
        }
        return childList(draft, parent, path);
      },
    },
    {
      id: "down",
      label: "Move later",
      run: () => {
        if (index + 1 < parent.children.length) {
          parent.children.splice(index, 1);
          parent.children.splice(index + 1, 0, child);
        }
        return childList(draft, parent, path);
      },
    },
    { id: "save", label: "Review and apply draft…", run: () => draft.finish() },
  ]);
}
// oxlint-disable-next-line eslint/max-lines-per-function -- Keep each form definition with the callbacks that own its draft.
function nodeForm(
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Submission updates the shared draft.
  draft: Draft,
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Submission replaces this draft node's fields.
  target: Node,
  path: readonly string[],
  done: () => PaletteView,
): PaletteView {
  if (path[0] === "themes" && target.children.length === 0 && target.arguments.length > 0)
    return colorForm(target, done, path[2] !== "terminal");
  const local = clone(target);
  const keys = Object.keys(local.properties);
  const fields: PaletteField[] = [
    { type: "text", id: "name", label: "Name", initialValue: local.name },
  ];
  const themeColor = path[0] === "themes" && local.children.length === 0;
  for (const [index, value] of local.arguments.entries()) {
    fields.push(
      scalarField(
        `arg:${index}`,
        themeColor
          ? `Color ${index + 1} (indexed 0–255, RGB components, or hex)`
          : local.name === "bind"
            ? "Key chord"
            : `Value ${index + 1}`,
        value,
        local.name === "bind" ? "key" : local.name === "cwd" ? "directory" : undefined,
      ),
    );
  }
  for (const key of keys) fields.push(scalarField(`prop:${key}`, key, local.properties[key] ?? ""));
  if (fields.length > 16)
    return list("Fields", () =>
      fields.map((field) => ({
        id: field.id,
        label: field.label,
        run: () => ({
          type: "form",
          title: field.label,
          fields: [field],
          onSubmit: (values) => {
            assign(values);
            Object.assign(target, local);
            return done();
          },
        }),
      })),
    );
  function assign(values: Readonly<Record<string, string>>): void {
    if (values["name"] !== undefined) {
      if (!values["name"].trim()) throw new Error("Name is required");
      local.name = values["name"];
    }
    local.arguments = local.arguments.map((v, i) =>
      values[`arg:${i}`] === undefined ? v : parseScalar(values[`arg:${i}`] ?? "", v),
    );
    for (const key of keys)
      if (values[`prop:${key}`] !== undefined)
        local.properties[key] = parseScalar(
          values[`prop:${key}`] ?? "",
          local.properties[key] ?? "",
        );
  }
  return {
    type: "form",
    title: clean(path.join(" / ")),
    fields,
    submitLabel: "Keep in draft",
    onSubmit: (values) => {
      assign(values);
      const parent =
        path[0] === "keybinds" && path.length === 3
          ? draft.value.children.find((mode) => nodeKey(mode) === path[1])
          : undefined;
      const conflicts =
        local.name === "bind"
          ? (parent?.children.filter(
              (binding) =>
                binding !== target &&
                binding.name === "bind" &&
                binding.arguments.some((chord) => local.arguments.includes(chord)),
            ) ?? [])
          : [];
      const apply = (): PaletteView => {
        Object.assign(target, local);
        if (parent && conflicts.length > 0) {
          for (const binding of conflicts)
            binding.arguments = binding.arguments.filter(
              (chord) => !local.arguments.includes(chord),
            );
          parent.children = parent.children.filter(
            (binding) => binding.name !== "bind" || binding.arguments.length > 0,
          );
        }
        return done();
      };
      if (local.name === "bind")
        return confirm(
          `Replace binding ${clean(local.arguments.join(" "))}? Existing actions: ${[target, ...conflicts].flatMap((binding) => binding.children.map((action) => action.name)).join(", ")}`,
          apply,
        );
      return apply();
    },
    actions: [
      {
        id: "add-argument",
        label: "Add argument…",
        run: (values) => {
          assign(values);
          return newValue("Argument", (name, value) => {
            void name;
            local.arguments.push(value);
            return nodeForm(draft, local, path, () => {
              Object.assign(target, local);
              return done();
            });
          });
        },
      },
      {
        id: "add-property",
        label: "Add property…",
        run: (values) => {
          assign(values);
          const properties = draft.snapshot.action_properties[local.name];
          if (properties)
            return list("Action properties", () =>
              Object.entries(properties).map(([name, initial]): PaletteAction => ({
                id: name,
                label: name,
                run: () => ({
                  type: "form",
                  title: `${local.name}: ${name}`,
                  fields: [scalarField("value", name, local.properties[name] ?? initial)],
                  onSubmit: (propertyValues) => {
                    local.properties[name] = parseScalar(propertyValues["value"] ?? "", initial);
                    return nodeForm(draft, local, path, () => {
                      Object.assign(target, local);
                      return done();
                    });
                  },
                }),
              })),
            );
          return newValue("Property", (name, value) => {
            local.properties[name] = value;
            return nodeForm(draft, local, path, () => {
              Object.assign(target, local);
              return done();
            });
          });
        },
      },
      {
        id: "remove-value",
        label: "Remove argument or property…",
        run: (values) => {
          assign(values);
          return list("Remove value", () => [
            ...local.arguments.map((_, i) => ({
              id: `arg:${i}`,
              label: `Argument ${i + 1}`,
              run: () => {
                local.arguments.splice(i, 1);
                return nodeForm(draft, local, path, () => {
                  Object.assign(target, local);
                  return done();
                });
              },
            })),
            ...Object.keys(local.properties).map((key) => ({
              id: `prop:${key}`,
              label: key,
              run: () => {
                local.properties = Object.fromEntries(
                  Object.entries(local.properties).filter(([name]) => name !== key),
                );
                return nodeForm(draft, local, path, () => {
                  Object.assign(target, local);
                  return done();
                });
              },
            })),
          ]);
        },
      },
    ],
  };
}
function newValue(title: string, done: (name: string, value: Scalar) => PaletteView): PaletteView {
  return {
    type: "form",
    title,
    fields: [
      { type: "text", id: "name", label: "Name", initialValue: "value" },
      { type: "choice", id: "type", label: "Type", options: ["Text", "Number", "Boolean"] },
      {
        type: "multiline",
        id: "value",
        label: "Value",
        help: "Shift+Enter inserts a newline. Boolean values are true or false.",
      },
    ],
    onSubmit: (values) => {
      const type = values["type"],
        value = values["value"] ?? "";
      if (type === "Boolean" && !["true", "false"].includes(value))
        throw new Error("Enter true or false");
      return done(
        values["name"] ?? "value",
        type === "Boolean" ? value === "true" : type === "Number" ? parseScalar(value, 0) : value,
      );
    },
  };
}
function discoveryDraft(snapshot: Snapshot, client: number, root: string): Draft {
  const fresh = read();
  if (fresh.revision !== snapshot.revision)
    throw new Error("Configuration changed. Search again before selecting an entry.");
  return new Draft(fresh, client, root);
}
// oxlint-disable-next-line eslint/max-lines-per-function -- Keep each form definition with the callbacks that own its draft.
function rootActions(snapshot: Snapshot, client: number): PaletteAction[] {
  const result: PaletteAction[] = snapshot.catalog.map((setting) => ({
    id: setting.name,
    label: `Configure ${setting.label}…`,
    aliases: [
      setting.name,
      "configuration settings",
      ...(setting.name.startsWith("copy_") ? ["clipboard"] : []),
    ],
    description: setting.timing,
    run: () => settingForm(new Draft(read(), client, setting.name), setting),
  }));
  for (const root of snapshot.sections) {
    result.push({
      id: root,
      label: `Configure ${root.replaceAll("_", " ")}…`,
      aliases: [root, "configuration settings", ...(root === "keybinds" ? ["keybindings"] : [])],
      run: () => {
        const draft = new Draft(read(), client, root);
        return childList(draft, draft.value, [root]);
      },
    });
    for (const [index, child] of (
      find(snapshot.clients[String(client)] ?? snapshot.saved, [root])?.children ?? []
    ).entries())
      result.push({
        id: `${root}:${index}:${child.name}`,
        label: `Configure ${root}: ${clean(child.name)}…`,
        aliases: [`${root} ${clean(child.name)}`],
        run: () => {
          const draft = discoveryDraft(snapshot, client, root);
          return entry(draft, draft.value, index, [root]);
        },
      });
  }
  for (const [modeIndex, mode] of (
    find(snapshot.clients[String(client)] ?? snapshot.saved, ["keybinds"])?.children ?? []
  ).entries()) {
    for (const [index, binding] of mode.children.entries()) {
      if (binding.name !== "bind") continue;
      const identity = nodeKey(binding);
      result.push({
        id: `keybinds:${modeIndex}:${mode.name}:${index}:${identity}`,
        label: `Configure keybinding ${clean(mode.name)}: ${clean(binding.arguments.join(" "))}…`,
        aliases: ["keybindings keybinds", ...binding.children.map((n) => n.name)],
        run: () => {
          const draft = discoveryDraft(snapshot, client, "keybinds");
          const parent = draft.value.children[modeIndex];
          return parent
            ? entry(draft, parent, index, ["keybinds", mode.name])
            : childList(draft, draft.value, ["keybinds"]);
        },
      });
    }
  }
  return result;
}
export default definePlugin({
  load() {
    yinz.commandPalette.registerProvider({
      id: "configuration",
      title: "Configuration",
      provideActions: (request) => {
        const words = request.query.toLowerCase().split(/\s+/u).filter(Boolean);
        return rootActions(read(), request.clientId)
          .filter((a) =>
            words.every((w) =>
              `${a.label} ${(a.aliases ?? []).join(" ")}`.toLowerCase().includes(w),
            ),
          )
          .slice(0, 100);
      },
    });
  },
});

// oxlint-disable-next-line eslint/max-lines-per-function, typescript/prefer-readonly-parameter-types -- Keep the form with its callbacks; Color submission updates this draft node.
function colorForm(target: Node, done: () => PaletteView, indexed: boolean): PaletteView {
  const format =
    typeof target.arguments[0] === "string"
      ? "Hex"
      : target.arguments.length === 3
        ? "RGB"
        : "Indexed";
  return {
    type: "form",
    title: target.name,
    fields: [
      {
        type: "choice",
        id: "format",
        label: "Color format",
        options: indexed ? ["Indexed", "RGB", "Hex"] : ["RGB", "Hex"],
        initialValue: format,
      },
      {
        type: "number",
        id: "index",
        label: "Index (0–255)",
        minimum: 0,
        maximum: 255,
        initialValue: format === "Indexed" ? String(target.arguments[0]) : "0",
      },
      ...["red", "green", "blue"].map((id, i): PaletteField => ({
        type: "number",
        id,
        label: id,
        minimum: 0,
        maximum: 255,
        initialValue: format === "RGB" ? String(target.arguments[i]) : "0",
      })),
      {
        type: "text",
        id: "hex",
        label: "Hex (#RRGGBB)",
        initialValue: format === "Hex" ? String(target.arguments[0]) : "#000000",
      },
    ],
    submitLabel: "Keep in draft",
    onSubmit: (values) => {
      if (values["format"] === "Hex") {
        if (!/^#[0-9a-f]{6}$/iu.test(values["hex"] ?? ""))
          throw new Error("Enter a six-digit color such as #12abef");
        target.arguments = [values["hex"] ?? ""];
      } else {
        target.arguments = (values["format"] === "RGB" ? ["red", "green", "blue"] : ["index"]).map(
          (id) => {
            const value = parseScalar(values[id] ?? "", 0);
            if (typeof value !== "number" || value < 0 || value > 255)
              throw new Error("Color values must be between 0 and 255");
            return value;
          },
        );
      }
      return done();
    },
  };
}
