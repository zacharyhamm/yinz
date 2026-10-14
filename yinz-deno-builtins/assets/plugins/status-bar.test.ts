import z, { type Action, type BareKey, type KeyWithModifier, type TabInfo } from "yinz:plugin";
import { test, loadPlugin, assert, assertEquals } from "yinz:testing";
import { defaultMode } from "./status-bar/default-mode.ts";

const activeTab: TabInfo = {
  position: 0,
  name: "main",
  active: true,
  panes_to_hide: 0,
  is_fullscreen_active: false,
  is_sync_panes_active: false,
  are_floating_panes_visible: false,
  other_focused_clients: [],
  active_swap_layout_name: null,
  is_swap_layout_dirty: false,
  viewport_rows: 24,
  viewport_columns: 80,
  display_area_rows: 24,
  display_area_columns: 80,
  selectable_tiled_panes_count: 1,
  selectable_floating_panes_count: 0,
  tab_id: 0,
  has_bell_notification: false,
  is_flashing_bell: false,
};

const replies = { GetLocalUtcOffset: 0, GetYinzVersion: "test", FsMetadata: null, FsWrite: null };
const named = (commands: unknown[], name: string) =>
  commands.filter((c) => typeof c === "object" && c !== null && name in c);

for (const failure of ["FsMetadata", "FsRead", "FsWrite", "malformed", "invalid counts"]) {
  test(`status bar keeps drawing when tip cache fails: ${failure}`, async () => {
    const cache =
      failure === "malformed"
        ? "{"
        : failure === "invalid counts"
          ? '{"yinz_version":"test","cached_data":{"quicknav":-1}}'
          : '{"yinz_version":"test","cached_data":{"quicknav":1}}';
    const p = await loadPlugin("./status-bar.ts", {
      replies: {
        ...replies,
        FsMetadata: {},
        FsRead: () => z.encodeUtf8(cache),
        ...(["FsMetadata", "FsRead", "FsWrite"].includes(failure)
          ? {
              [failure]: () => {
                throw Error("cache unavailable");
              },
            }
          : {}),
      },
    });
    const frame = await p.render(1, 80);
    assert(frame.includes("00:00"));
    await p.event({ TabUpdate: [activeTab] });
    assert((await p.render(2, 200)).includes("open new pane"));
    assertEquals(named(p.commands, "SetTimeout"), [{ SetTimeout: 60 }]);
  });
}
for (const [version, hits, expected] of [
  ["test", 20, 21],
  ["old", 300, 2],
] as const) {
  test(`status bar tip cache threshold and version: ${version}/${hits}`, async () => {
    const p = await loadPlugin("./status-bar.ts", {
      replies: {
        ...replies,
        FsMetadata: {},
        FsRead: () =>
          z.encodeUtf8(JSON.stringify({ yinz_version: version, cached_data: { quicknav: hits } })),
      },
    });
    const writes = p.commands.filter(
      (c): c is { FsWrite: { contents: number[] } } =>
        typeof c === "object" && c !== null && "FsWrite" in c,
    );
    assertEquals(writes.length, 1, JSON.stringify(p.commands));
    const contents = JSON.parse(z.decodeUtf8(new Uint8Array(writes[0]!.FsWrite.contents)));
    assertEquals(contents.yinz_version, "test");
    assertEquals(contents.cached_data.quicknav, expected);
  });
}

test("status bar deduplicates timers through repeated Alt and pointer reveals", async () => {
  const p = await loadPlugin("./status-bar.ts", { replies });
  await p.render(1, 80);
  for (let i = 0; i < 20; i++) {
    await p.event({ ModifiersHeld: { modifiers: ["Alt"], is_live: false } });
    await p.event({ Mouse: { Hover: [0, 0] } });
    await p.event({ Mouse: { Hover: [0, 65535] } });
  }
  assertEquals(named(p.commands, "SetTimeout"), [{ SetTimeout: 60 }, { SetTimeout: 3 }]);
  await p.advance(3000);
  assertEquals(named(p.commands, "SetTimeout").at(-1), { SetTimeout: 57 });
  // A late, superseded timeout must not start another permanent timer chain.
  await p.advance(1000);
  assertEquals(named(p.commands, "SetTimeout").length, 3);
  await p.advance(56000);
  assertEquals(named(p.commands, "SetTimeout").length, 4);
  assert((await p.render(1, 80)).includes("00:01"));
});

test("status bar refreshes the UTC offset at the next whole hour", async () => {
  const p = await loadPlugin("./status-bar.ts", {
    replies: { ...replies, GetLocalUtcOffset: 19800 },
  });
  assert((await p.render(1, 80)).includes("05:30"));
  await p.advance(3599999);
  assertEquals(named(p.commands, "GetLocalUtcOffset").length, 1);
  p.stub("GetLocalUtcOffset", -18000);
  await p.advance(1);
  assertEquals(named(p.commands, "GetLocalUtcOffset").length, 2);
  assert((await p.render(1, 80)).includes("20:00"));
});

// Ctrl e o moves the agents panel by sending the running plugins a message, not by launching one,
// so its key is found by the message's name - another message bound in the mode is not it.
test("status bar names the Agent-mode key that moves the panel by its message", async () => {
  const p = await loadPlugin("./status-bar.ts", { replies, configuration: { classic: "true" } });
  const pipe = (name: string): Action => ({
    KeybindPipe: {
      name,
      payload: null,
      args: null,
      plugin: null,
      plugin_id: null,
      configuration: null,
      launch_new: false,
      skip_cache: false,
      floating: false,
      in_place: null,
      cwd: null,
      pane_title: null,
    },
  });
  const bind = (char: string, name: string): [KeyWithModifier, Action[]] => [
    { bare_key: { Char: char }, key_modifiers: [] },
    [pipe(name), { SwitchToMode: { input_mode: "Normal" } }],
  ];
  const done: [KeyWithModifier, Action[]] = [
    { bare_key: "Enter", key_modifiers: [] },
    [{ SwitchToMode: { input_mode: "Normal" } }],
  ];
  const agentMode = (...binds: [KeyWithModifier, Action[]][]) => ({
    ...defaultMode(),
    mode: "Agent" as const,
    keybinds: [["Agent", [...binds, done]] as ["Agent", [KeyWithModifier, Action[]][]]],
  });
  await p.event({ TabUpdate: [activeTab] });
  await p.event({ ModeUpdate: agentMode(bind("m", "something_else")) });
  const without = await p.render(2, 200);
  assert(without.includes("Select pane") && !without.includes("Move panel"), without);
  await p.event({
    ModeUpdate: agentMode(bind("m", "something_else"), bind("o", "agent_strip_reposition")),
  });
  const frame = await p.render(2, 200);
  assert(frame.includes("<o> Move panel"), frame);
});

// Tab focus names the arrow pointing each way even when the binding also leaves the mode, and
// even though Up and Down, bound the same, sort before Right.
test("status bar names the Tab-mode arrows for bindings that also leave the mode", async () => {
  const p = await loadPlugin("./status-bar.ts", { replies, configuration: { classic: "true" } });
  const toNormal: Action = { SwitchToMode: { input_mode: "Normal" } };
  const bind = (bare_key: BareKey, action: Action): [KeyWithModifier, Action[]] => [
    { bare_key, key_modifiers: [] },
    [action, toNormal],
  ];
  const binds: [KeyWithModifier, Action[]][] = [
    bind("Left", "GoToPreviousTab"),
    bind("Up", "GoToPreviousTab"),
    bind({ Char: "h" }, "GoToPreviousTab"),
    bind("Right", "GoToNextTab"),
    bind("Down", "GoToNextTab"),
    bind({ Char: "l" }, "GoToNextTab"),
    [{ bare_key: "Enter", key_modifiers: [] }, [toNormal]],
  ];
  await p.event({ TabUpdate: [activeTab] });
  await p.event({
    ModeUpdate: {
      ...defaultMode(),
      mode: "Tab" as const,
      keybinds: [["Tab", binds] as ["Tab", [KeyWithModifier, Action[]][]]],
    },
  });
  const frame = await p.render(2, 200);
  assert(frame.includes("<←→> Change focus"), frame);
});

test("status bar clipboard messages clear on input and live Alt release restores the clock", async () => {
  const p = await loadPlugin("./status-bar.ts", { replies });
  const mode = defaultMode();
  mode.keybinds = [
    [
      "Normal",
      [
        [
          { bare_key: { Char: "n" }, key_modifiers: ["Alt"] },
          [{ NewPane: { direction: null, pane_name: null, start_suppressed: false } }],
        ],
        [
          { bare_key: { Char: "p" }, key_modifiers: ["Ctrl"] },
          [{ SwitchToMode: { input_mode: "Pane" } }],
        ],
      ],
    ],
  ];
  await p.event({ ModeUpdate: mode });
  await p.event({ ModifiersHeld: { modifiers: ["Alt"], is_live: true } });
  assert((await p.render(1, 200)).includes("New Pane"));
  await p.event({ ModifiersHeld: { modifiers: [], is_live: true } });
  assert((await p.render(1, 200)).includes("00:00"));
  await p.event({ CopyToClipboard: "System" });
  assert((await p.render(1, 200)).includes("Text copied to system clipboard"));
  await p.event("SystemClipboardFailure");
  assert((await p.render(1, 200)).includes("Text copied to system clipboard"));
  await p.event("InputReceived");
  assert((await p.render(1, 200)).includes("00:00"));
  await p.event("SystemClipboardFailure");
  assert((await p.render(1, 200)).includes("Error using the system clipboard"));
});
