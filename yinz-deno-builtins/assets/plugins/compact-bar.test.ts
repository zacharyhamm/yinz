import { type Keybinds, type PaneInfo, type TabInfo } from "yinz:plugin";
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
  viewport_columns: 120,
  display_area_rows: 24,
  display_area_columns: 120,
  selectable_tiled_panes_count: 1,
  selectable_floating_panes_count: 0,
  tab_id: 0,
  has_bell_notification: false,
  is_flashing_bell: false,
};
const replies = {
  GetPluginIds: { plugin_id: 7, client_id: 3, hostname: "host" },
  SwitchTabTo: null,
  FocusHostSession: null,
  Reconfigure: null,
  MessageToPlugin: null,
  ChangeFloatingPanesCoordinates: null,
  RenamePluginPane: null,
  BreakPanesToTabWithIndex: null,
  CloseSelf: null,
};
const named = (commands: unknown[], name: string) =>
  commands.filter((command) => typeof command === "object" && command !== null && name in command);

test("compact bar clipboard messages take precedence and clear on input", async () => {
  const p = await loadPlugin("./compact-bar.ts", { replies });
  assertEquals(await p.render(1, 120), "");
  await p.event({ TabUpdate: [activeTab] });
  const frame = () => p.render(1, 120);
  assert((await frame()).includes("main"));
  for (const [destination, text] of [
    ["System", "Text copied to system clipboard"],
    ["Primary", "Text copied to system primary selection"],
    ["Command", "Text piped to external command"],
  ] as const) {
    await p.event({ CopyToClipboard: destination });
    await p.event("SystemClipboardFailure");
    assert((await frame()).includes(text));
    await p.event("InputReceived");
    assert((await frame()).includes("main"));
  }
  await p.event("SystemClipboardFailure");
  assert((await frame()).includes("Error using the system clipboard"));
  await p.event("InputReceived");
  await p.event({ TabUpdate: [{ ...activeTab, name: "ignored", active: false }] });
  assert((await frame()).includes("main"));
});

test("compact bar clicks and wheel scrolling preserve one-based tab navigation", async () => {
  const p = await loadPlugin("./compact-bar.ts", { replies });
  await p.event({
    TabUpdate: [activeTab, { ...activeTab, position: 1, tab_id: 1, name: "second", active: false }],
  });
  const frame = await p.render(1, 120);
  assert(frame.includes("second"));
  await p.event({ Mouse: { LeftClick: [0, frame.indexOf("main")] } });
  assertEquals(named(p.commands, "SwitchTabTo"), []);
  await p.event({ Mouse: { LeftClick: [0, frame.indexOf("second")] } });
  await p.event({ Mouse: { ScrollUp: 0 } });
  await p.event({ Mouse: { ScrollDown: 0 } });
  assertEquals(named(p.commands, "SwitchTabTo"), [
    { SwitchTabTo: 2 },
    { SwitchTabTo: 2 },
    { SwitchTabTo: 1 },
  ]);
  await p.event({
    ModeUpdate: {
      ...defaultMode(),
      session_name: "child",
      session_ancestry: ["parent"],
      host_fullscreen: true,
    },
  });
  const nested = await p.render(1, 120);
  assert(nested.includes("parent"));
  await p.event({ Mouse: { LeftClick: [0, nested.indexOf("parent")] } });
  assertEquals(p.commands.at(-1), "FocusHostSession");
});

test("compact bar launches persisted tooltips only for its own client and active tab", async () => {
  const p = await loadPlugin("./compact-bar.ts", { replies, configuration: { tooltip: "Ctrl y" } });
  assertEquals(named(p.commands, "Reconfigure").length, 1);
  await p.event({ TabUpdate: [activeTab] });
  const pane: PaneInfo = {
    id: 7,
    is_plugin: true,
    is_focused: false,
    is_fullscreen: false,
    is_floating: false,
    is_suppressed: false,
    is_stack_list_member: false,
    title: "compact-bar",
    exited: false,
    exit_status: null,
    is_held: false,
    restart_error: null,
    pane_x: 0,
    pane_content_x: 0,
    pane_y: 0,
    pane_content_y: 0,
    pane_rows: 1,
    pane_content_rows: 1,
    pane_columns: 120,
    pane_content_columns: 120,
    cursor_coordinates_in_pane: null,
    terminal_command: null,
    plugin_url: "yinz:compact-bar",
    is_selectable: false,
    index_in_pane_group: {},
    default_fg: null,
    default_bg: null,
    cwd: null,
    host: null,
    target: "Local",
    agent: null,
  };
  await p.event({ PaneUpdate: { panes: { 0: [pane] } } });
  await p.pipe({ name: "toggle_tooltip", payload: "3", is_private: false });
  await p.pipe({ name: "toggle_tooltip", payload: "4" });
  await p.event({ PaneUpdate: { panes: { 1: [pane] } } });
  await p.pipe({ name: "toggle_tooltip", payload: "3" });
  assertEquals(named(p.commands, "MessageToPlugin"), []);
  await p.event({ PaneUpdate: { panes: { 0: [pane] } } });
  await p.pipe({ name: "toggle_tooltip", payload: "3" });
  const messages = named(p.commands, "MessageToPlugin");
  assertEquals(messages.length, 1);
  const message = JSON.stringify(messages[0]);
  assert(message.includes('"message_name":"toggle_persisted_tooltip"'));
  assert(message.includes('"is_tooltip":"true"'));
  assert(message.includes('"persist":""'));
  await p.event({ ModeUpdate: { ...defaultMode(), mode: "Pane" } });
  assertEquals(named(p.commands, "MessageToPlugin").length, 2);
  await p.event({ ModeUpdate: { ...defaultMode(), mode: "Locked" } });
  await p.event({ ModeUpdate: defaultMode() });
  assertEquals(named(p.commands, "MessageToPlugin").length, 2);
});

test("compact tooltip retains initial keys, follows tabs, and closes in normal mode", async () => {
  const p = await loadPlugin("./compact-bar.ts", {
    replies,
    configuration: { is_tooltip: "true" },
  });
  const keybinds: Keybinds = [
    ["Pane", [[{ bare_key: { Char: "f" }, key_modifiers: [] }, ["ToggleFocusFullscreen"]]]],
  ];
  await p.event({ InitialKeybinds: keybinds });
  await p.event({ ModeUpdate: { ...defaultMode(), mode: "Pane" } });
  assert((await p.render(10, 80)).includes("Toggle fullscreen"));
  await p.event({ TabUpdate: [activeTab] });
  await p.event({
    TabUpdate: [
      { ...activeTab, active: false },
      { ...activeTab, position: 1, tab_id: 1 },
    ],
  });
  assertEquals(named(p.commands, "BreakPanesToTabWithIndex").at(-1), {
    BreakPanesToTabWithIndex: [[{ Plugin: 7 }], 1, false],
  });
  await p.event({ Mouse: { ScrollUp: 0 } });
  assertEquals(named(p.commands, "SwitchTabTo"), []);
  await p.event({ ModeUpdate: defaultMode() });
  assert(p.commands.includes("CloseSelf"));
});

test("compact tooltip persists across modes until toggled again", async () => {
  const persisted = await loadPlugin("./compact-bar.ts", {
    replies,
    configuration: { is_tooltip: "true" },
  });
  await persisted.pipe({ name: "toggle_persisted_tooltip" });
  await persisted.event({ ModeUpdate: defaultMode() });
  await persisted.event({ ModeUpdate: { ...defaultMode(), mode: "RenamePane" } });
  assert(!persisted.commands.includes("CloseSelf"));
  assert((await persisted.render(10, 80)).includes("Renaming pane..."));
  await persisted.pipe({ name: "toggle_persisted_tooltip" });
  assertEquals(persisted.commands.at(-1), "CloseSelf");
});
