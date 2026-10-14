import { test, loadPlugin, assert, assertEquals, type PaletteTestReply } from "yinz:testing";
import type { ConfigurationSnapshot } from "yinz:plugin";
const snapshot: ConfigurationSnapshot = {
  action_properties: {},
  action_children: {},
  section_timing: {},
  sections: [],
  templates: {},
  revision: "revision-one",
  notice: "Applied to this session",
  overrides: [],
  actions: [],
  catalog: [
    {
      name: "mouse_mode",
      label: "mouse mode",
      kind: "boolean",
      choices: ["Enabled", "Disabled"],
      minimum: null,
      maximum: null,
      help: "Mouse",
      timing: "Applies to this session",
    },
  ],
  saved: [{ name: "mouse_mode", arguments: [true], properties: {}, children: [] }],
  clients: { "1": [{ name: "mouse_mode", arguments: [true], properties: {}, children: [] }] },
};
async function harness(query = "mouse_mode", inspected = snapshot, id: string | null = query) {
  const p = await loadPlugin("./configuration-palette.ts", {
    replies: {
      RequestPluginPermissions: null,
      CommandPalette: null,
      InspectConfiguration: inspected,
      EditConfiguration: snapshot,
    },
  });
  await p.event({ PermissionRequestResult: "Granted" });
  const registration = p.paletteCommands.find((c) => c.type === "register");
  assert(registration?.type === "register");
  let scope = { registration: registration.registration, opening: 1, view: 0, revision: 1 };
  const context = { clientId: 1, paneId: { Terminal: 1 }, tabId: 1, currentProject: null };
  function reply(): PaletteTestReply {
    const command = p.paletteCommands.at(-1);
    assert(command?.type === "reply");
    if ("Err" in command.result) throw new Error(command.result.Err);
    return command.result.Ok;
  }
  async function execute(handle: string, values: Record<string, string> | null = null) {
    scope = { ...scope, revision: scope.revision + 1 };
    await p.palette({ type: "execute", scope, context, handle, values }, 1);
    const result = reply();
    if (result.type === "view") scope = { ...scope, view: scope.view + 1 };
    if (result.type === "view" && result.view.type === "actions" && result.view.source) {
      scope = { ...scope, revision: scope.revision + 1 };
      await p.palette(
        { type: "discover", scope, context, query: "", source: result.view.source },
        1,
      );
      const discovered = reply();
      assert(discovered.type === "actions");
      return { ...result, view: { ...result.view, actions: discovered.actions } };
    }
    return result;
  }
  await p.palette({ type: "discover", scope, context, query: "", source: null }, 1);
  const main = reply();
  assert(main.type === "actions");
  assertEquals(
    main.actions.map((a) => a.label),
    ["Configuration…"],
  );
  const submenu = await execute(main.actions[0]!.handle);
  assert(submenu.type === "view" && submenu.view.type === "actions");
  const source = submenu.view.source;
  async function search(query: string) {
    scope = { ...scope, revision: scope.revision + 1 };
    await p.palette({ type: "discover", scope, context, query, source }, 1);
    const roots = reply();
    assert(roots.type === "actions");
    assert(roots.actions.every((a) => !a.label.startsWith("Configure ")));
    return roots;
  }
  const roots = await search(query);
  const action = roots.actions.find((a) => a.id === id);
  assert(action || id === null);
  return { p, execute, action: action!, roots, search };
}
for (const destination of ["Apply to session", "Save permanently"]) {
  test(`scalar editor submits value and destination together: ${destination}`, async () => {
    const { p, execute, action } = await harness();
    const form = await execute(action.handle);
    assert(form.type === "view" && form.view.type === "form");
    assert(form.view.title.includes("Client 1"));
    assertEquals(form.view.submitLabel, "Apply change");
    assertEquals(
      form.view.fields.map((f) => f.initialValue),
      ["Enabled", "Apply to session"],
    );
    assert(!p.commands.some((c) => JSON.stringify(c).includes("EditConfiguration")));
    const completion = await execute(form.view.handle, { value: "Disabled", destination });
    assert(completion.type === "complete");
    const writes = p.commands.filter((c) => JSON.stringify(c).includes("EditConfiguration"));
    assertEquals(writes, [
      {
        EditConfiguration: {
          revision: snapshot.revision,
          destination: destination === "Save permanently" ? "permanent" : "session",
          patches: [{ path: ["mouse_mode"], node: { ...snapshot.saved[0]!, arguments: [false] } }],
          reset: [],
        },
      },
    ]);
  });
}
test("opening an editor and cancelling does not write configuration", async () => {
  const { p, execute, action } = await harness();
  await execute(action.handle);
  await p.palette({ type: "dismiss", opening: 1 }, 1);
  assert(!p.commands.some((c) => JSON.stringify(c).includes("EditConfiguration")));
});

test("ordered actions stay in a draft and save in the selected order", async () => {
  const actionNode = (name: string) => ({ name, arguments: [], properties: {}, children: [] });
  const binding = {
    ...actionNode("bind"),
    arguments: ["Ctrl x"],
    children: [actionNode("Copy"), actionNode("Quit")],
  };
  const root = {
    ...actionNode("keybinds"),
    children: [{ ...actionNode("normal"), children: [binding] }],
  };
  const inspected = {
    ...snapshot,
    sections: ["keybinds"],
    clients: { "1": [root] },
    saved: [root],
  };
  const { p, execute, action } = await harness("keybinds", inspected);
  let reply = await execute(action.handle);
  async function choose(id: string) {
    assert(reply.type === "view" && reply.view.type === "actions");
    const selected = reply.view.actions.find((a) => a.id === id);
    assert(selected, id);
    reply = await execute(selected.handle);
  }
  await choose("entry:0");
  await choose("children");
  await choose("entry:0");
  await choose("children");
  await choose("entry:0");
  await choose("down");
  assert(!p.commands.some((c) => JSON.stringify(c).includes("EditConfiguration")));
  await choose("save");
  assert(reply.type === "view" && reply.view.type === "form");
  await execute(reply.view.handle, { destination: "Apply to session" });
  const write = p.commands.find((c) => JSON.stringify(c).includes("EditConfiguration"));
  assert(write);
  const encoded = JSON.stringify(write);
  assert(encoded.indexOf('"name":"Quit"') < encoded.indexOf('"name":"Copy"'));
});

const emptyNode = (name: string, children: ConfigurationSnapshot["saved"] = []) => ({
  name,
  arguments: [],
  properties: {},
  children,
});
for (const id of ["keybinds:1:shared_among", 'keybinds:1:shared_among:0:bind:["Ctrl a"]']) {
  test(`discovery selects repeated entry ${id} by position`, async () => {
    const groups = ["normal", "locked"].map((mode) => ({
      ...emptyNode("shared_among"),
      arguments: [mode],
      children: [
        {
          ...emptyNode("bind", [emptyNode(mode === "normal" ? "Quit" : "Copy")]),
          arguments: ["Ctrl a"],
        },
      ],
    }));
    const root = emptyNode("keybinds", groups);
    const inspected = {
      ...snapshot,
      sections: ["keybinds"],
      saved: [root],
      clients: { "1": [root] },
    };
    const { execute, action, roots } = await harness("shared_among", inspected, id);
    assertEquals(new Set(roots.actions.map((a) => a.id)).size, roots.actions.length);
    const opened = await execute(action.handle);
    assert(opened.type === "view" && opened.view.type === "actions");
    const selected = opened.view.actions.find(
      (a) => a.id === (id.includes("bind:") ? "children" : "edit"),
    );
    assert(selected);
    const fields = await execute(selected.handle);
    if (id.includes("bind:")) {
      assert(fields.type === "view" && fields.view.type === "actions");
      assert(fields.view.actions.some((a) => a.label.includes("Copy")));
    } else {
      assert(fields.type === "view" && fields.view.type === "form");
      assert(fields.view.fields.some((f) => f.initialValue === "locked"));
    }
  });
}
test("discovery selects the second separator and rejects stale selections", async () => {
  const root = emptyNode("context_menu", [
    emptyNode("separator"),
    emptyNode("item"),
    emptyNode("separator"),
  ]);
  const inspected = {
    ...snapshot,
    sections: ["context_menu"],
    saved: [root],
    clients: { "1": [root] },
  };
  const { p, execute, action, roots } = await harness(
    "separator",
    inspected,
    "context_menu:2:separator",
  );
  assertEquals(new Set(roots.actions.map((a) => a.id)).size, 2);
  let reply = await execute(action.handle);
  assert(reply.type === "view" && reply.view.type === "actions");
  const remove = reply.view.actions.find((a) => a.id === "remove");
  assert(remove);
  reply = await execute(remove.handle);
  assert(reply.type === "view" && reply.view.type === "form");
  reply = await execute(reply.view.handle);
  assert(reply.type === "view" && reply.view.type === "actions");
  const save = reply.view.actions.find((a) => a.id === "save");
  assert(save);
  reply = await execute(save.handle);
  assert(reply.type === "view" && reply.view.type === "form");
  await execute(reply.view.handle, { destination: "Apply to session" });
  const write = p.commands.find((c) => JSON.stringify(c).includes("EditConfiguration"));
  assert(write);
  assertEquals(write, {
    EditConfiguration: {
      revision: inspected.revision,
      destination: "session",
      patches: [
        {
          path: ["context_menu"],
          node: emptyNode("context_menu", [emptyNode("separator"), emptyNode("item")]),
        },
      ],
      reset: [],
    },
  });
  p.stub("InspectConfiguration", { ...inspected, revision: "revision-two" });
  let error = "";
  try {
    await execute(action.handle);
  } catch (caught) {
    error = String(caught);
  }
  assert(error.includes("Configuration changed"));
});
for (const destination of ["Apply to session", "Save permanently"]) {
  test(`unchanged session overrides: ${destination} after reload`, async () => {
    const effective = { ...emptyNode("mouse_mode"), arguments: [false] };
    const unrelated = { ...emptyNode("pane_frames"), arguments: [false] };
    const inspected = {
      ...snapshot,
      clients: { "1": [effective, unrelated] },
      saved: [...snapshot.saved, { ...unrelated, arguments: [true] }],
    };
    const { p, execute, action } = await harness("mouse_mode", inspected);
    let reply = await execute(action.handle);
    assert(reply.type === "view" && reply.view.type === "form");
    const reload = reply.view.actions?.find((a) => a.id === "reload");
    assert(reload);
    p.stub("InspectConfiguration", { ...snapshot, revision: "revision-two" });
    reply = await execute(reload.handle, { value: "Disabled", destination });
    assert(reply.type === "view" && reply.view.type === "form");
    assert(reply.view.confirmation);
    assert(!p.commands.some((c) => JSON.stringify(c).includes("EditConfiguration")));
    reply = await execute(reply.view.handle);
    assert(reply.type === "view" && reply.view.type === "form");
    assert(!p.commands.some((c) => JSON.stringify(c).includes("EditConfiguration")));
    await execute(reply.view.handle, { destination });
    const write = p.commands.find((c) => JSON.stringify(c).includes("EditConfiguration"));
    assert(write);
    assertEquals(write, {
      EditConfiguration: {
        revision: "revision-two",
        destination: destination === "Save permanently" ? "permanent" : "session",
        patches:
          destination === "Save permanently" ? [{ path: ["mouse_mode"], node: effective }] : [],
        reset: [],
      },
    });
  });
}

for (const add of [false, true]) {
  test(`permanent binding ${add ? "replacement" : "edit"} preserves unrelated overrides`, async () => {
    const binding = (chords: string[], action: string) => ({
      ...emptyNode("bind", [emptyNode(action)]),
      arguments: chords,
    });
    const original = binding(["Ctrl a"], "Quit");
    const temporary = binding(["Ctrl a"], "Copy");
    const occupied = binding(add ? ["Ctrl b", "Ctrl c"] : ["Ctrl b"], "Quit");
    const keybinds = (first: typeof original) =>
      emptyNode("keybinds", [emptyNode("normal", [first, occupied])]);
    const inspected = {
      ...snapshot,
      sections: ["keybinds"],
      templates: { "keybinds/*": [binding(["Ctrl b"], "Copy")] },
      saved: [keybinds(original)],
      clients: { "1": [keybinds(temporary)] },
    };
    const { p, execute, action } = await harness("keybinds", inspected);
    let reply = await execute(action.handle);
    async function choose(id: string) {
      assert(reply.type === "view" && reply.view.type === "actions");
      const selected = reply.view.actions.find((a) => a.id === id);
      assert(selected, id);
      reply = await execute(selected.handle);
    }
    await choose("entry:0");
    await choose("children");
    if (add) {
      await choose("add");
      await choose("template:0");
      assert(reply.type === "view" && reply.view.type === "form");
      reply = await execute(reply.view.handle, { name: "bind", "arg:0": "Ctrl b" });
      assert(reply.type === "view" && reply.view.type === "form");
      reply = await execute(reply.view.handle);
    } else {
      await choose("entry:1");
      await choose("children");
      await choose("entry:0");
      await choose("edit");
      assert(reply.type === "view" && reply.view.type === "form");
      reply = await execute(reply.view.handle, { name: "Copy" });
    }
    await choose("save");
    assert(reply.type === "view" && reply.view.type === "form");
    await execute(reply.view.handle, { destination: "Save permanently" });
    const write = p.commands.find((c) => JSON.stringify(c).includes("EditConfiguration"));
    assert(write);
    assertEquals(write, {
      EditConfiguration: {
        revision: inspected.revision,
        destination: "permanent",
        reset: [],
        patches: add
          ? [
              { path: ["keybinds", "normal", 'bind:["Ctrl b","Ctrl c"]'], node: null },
              {
                path: ["keybinds", "normal", 'bind:["Ctrl c"]'],
                node: binding(["Ctrl c"], "Quit"),
              },
              {
                path: ["keybinds", "normal", 'bind:["Ctrl b"]'],
                node: binding(["Ctrl b"], "Copy"),
              },
            ]
          : [
              {
                path: ["keybinds", "normal", 'bind:["Ctrl b"]'],
                node: binding(["Ctrl b"], "Copy"),
              },
            ],
      },
    });
  });
}

for (const [saved, effective, submitted] of [
  [true, false, "Enabled"],
  [false, true, "Enabled"],
] as const) {
  test(`permanent scalar save reconciles saved=${saved} and effective=${effective}`, async () => {
    const inspected = {
      ...snapshot,
      saved: [{ ...snapshot.saved[0]!, arguments: [saved] }],
      clients: { "1": [{ ...snapshot.saved[0]!, arguments: [effective] }] },
    };
    const { p, execute, action } = await harness("mouse_mode", inspected);
    const form = await execute(action.handle);
    assert(form.type === "view" && form.view.type === "form");
    await execute(form.view.handle, { value: submitted, destination: "Save permanently" });
    const write = p.commands.find((command) =>
      JSON.stringify(command).includes("EditConfiguration"),
    );
    assertEquals(write, {
      EditConfiguration: {
        revision: inspected.revision,
        destination: "permanent",
        patches: [
          {
            path: ["mouse_mode"],
            node: { name: "mouse_mode", arguments: [true], properties: {}, children: [] },
          },
        ],
        reset: [],
      },
    });
  });
}

test("configuration discovery excludes theme settings and descendants", async () => {
  const themeSettings = [
    "theme",
    "theme_dark",
    "theme_light",
    "theme_dir",
    "explicit_theme_hue",
    "agent_theme_mapping",
  ];
  const themes = emptyNode("themes", [emptyNode("custom-palette", [emptyNode("accent")])]);
  const inspected = {
    ...snapshot,
    catalog: [
      ...snapshot.catalog,
      ...themeSettings.map((name) => ({ ...snapshot.catalog[0]!, name, label: name })),
    ],
    sections: ["themes", "keybinds"],
    saved: [...snapshot.saved, themes],
    clients: { "1": [...snapshot.clients["1"]!, themes] },
  };
  const { roots, search } = await harness("", inspected, null);
  assertEquals(
    roots.actions.map((a) => a.id),
    ["mouse_mode", "keybinds"],
  );
  for (const query of ["theme", ...themeSettings, "themes", "custom-palette", "accent"]) {
    const roots = await search(query);
    assertEquals(roots.actions, [], query);
  }
});

test("startup-only scalar offers only permanent saving", async () => {
  const setting = { ...snapshot.catalog[0]!, timing: "On next session start" };
  const { p, execute, action } = await harness("mouse_mode", { ...snapshot, catalog: [setting] });
  const reply = await execute(action.handle);
  assert(reply.type === "view" && reply.view.type === "form");
  const destination = reply.view.fields.find((field) => field.id === "destination");
  assert(destination?.type === "choice");
  assertEquals(destination.options, ["Save permanently"]);
  assertEquals(destination.initialValue, "Save permanently");
  await execute(reply.view.handle, { value: "Disabled", destination: destination.initialValue! });
  assert(
    p.commands.some((command) => JSON.stringify(command).includes('"destination":"permanent"')),
  );
});

test("scalar submission retains the revision from opening the editor", async () => {
  const { p, execute, action } = await harness();
  const reply = await execute(action.handle);
  assert(reply.type === "view" && reply.view.type === "form");
  p.stub("InspectConfiguration", { ...snapshot, revision: "revision-two" });
  await execute(reply.view.handle, { value: "Disabled", destination: "Apply to session" });
  const write = p.commands.find((command) => JSON.stringify(command).includes("EditConfiguration"));
  assert(write);
  assert(JSON.stringify(write).includes('"revision":"revision-one"'));
});

for (const operation of ["unset", "reset"]) {
  test(`scalar ${operation} keeps its confirmation and destination workflow`, async () => {
    const { p, execute, action } = await harness();
    let reply = await execute(action.handle);
    assert(reply.type === "view" && reply.view.type === "form");
    const selected = reply.view.actions.find((a) => a.id === operation);
    assert(selected);
    reply = await execute(selected.handle);
    assert(reply.type === "view" && reply.view.type === "form");
    if (operation === "unset") {
      assert(reply.view.confirmation);
      assert(!p.commands.some((c) => JSON.stringify(c).includes("EditConfiguration")));
      reply = await execute(reply.view.handle);
      assert(reply.type === "view" && reply.view.type === "form");
    }
    assertEquals(
      reply.view.fields.map((field) => field.id),
      ["destination"],
    );
    assert(!p.commands.some((c) => JSON.stringify(c).includes("EditConfiguration")));
    await execute(reply.view.handle, { destination: "Apply to session" });
    assertEquals(
      p.commands.find((c) => JSON.stringify(c).includes("EditConfiguration")),
      {
        EditConfiguration: {
          revision: snapshot.revision,
          destination: "session",
          patches: operation === "unset" ? [{ path: ["mouse_mode"], node: null }] : [],
          reset: operation === "reset" ? [["mouse_mode"]] : [],
        },
      },
    );
  });
}

test("configuration submenu preserves clipboard and keybinding search aliases", async () => {
  const inspected = {
    ...snapshot,
    catalog: [{ ...snapshot.catalog[0]!, name: "copy_command", label: "copy command" }],
    sections: ["keybinds"],
  };
  const { search } = await harness("", inspected, null);
  for (const [query, id] of [
    ["clipboard", "copy_command"],
    ["keybindings", "keybinds"],
  ]) {
    const roots = await search(query!);
    assertEquals(
      roots.actions.map((a) => a.id),
      [id],
    );
  }
});
