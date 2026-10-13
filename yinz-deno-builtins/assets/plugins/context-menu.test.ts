import { test, loadPlugin, assertEquals } from "yinz:testing";
import type { ContextMenuContext, PaneId } from "yinz:plugin";

const source = {
  request_id: 10,
  pane_id: { Terminal: 7 },
  items: [{ id: 1, label: "Custom action" }],
  selected_text: null,
  link: null,
} as ContextMenuContext;

test("context-menu adds agent actions only for terminal agent records", async () => {
  const p = await loadPlugin("./context-menu.ts", {
    replies: { RunCommand: null, SetContextMenu: null, GetPaneInfo: null },
  });
  for (const pane of [null, { agent: null }, { agent: { kind: "codex", source: "Reported" } }]) {
    p.stub("GetPaneInfo", pane);
    for (const pane_id of [{ Terminal: 7 }, { Plugin: 7 }] as PaneId[]) {
      await p.event({ ContextMenuRequested: { ...source, pane_id } });
      const agent = "Terminal" in pane_id && pane?.agent;
      assertEquals(p.commands.at(-1), {
        SetContextMenu: {
          request_id: 10,
          items: [
            ...(agent
              ? [
                  { label: "Duplicate agent", context: { action: "duplicate_agent" } },
                  { label: "Fork agent", context: { action: "fork_agent" } },
                  { separator: true },
                ]
              : []),
            ...source.items,
          ],
        },
      });
    }
  }
});

test("context-menu agent selection retains clicked source and rechecks its record", async () => {
  const p = await loadPlugin("./context-menu.ts", {
    replies: { RunCommand: null, SetContextMenu: null, RunAction: null },
  });
  for (const action of ["duplicate_agent", "fork_agent"]) {
    p.stub("GetPaneInfo", { agent: { kind: "codex" } });
    await p.event({ ContextMenuRequested: source });
    // The focused pane is different when the menu item is selected.
    p.stub("GetFocusedPaneInfo", { pane_id: { Terminal: 99 }, tab_id: 1 });
    await p.event({
      ContextMenuItemSelected: { label: "", item_context: { action }, context: source },
    });
    assertEquals(p.commands.slice(-2), [
      { GetPaneInfo: { Terminal: 7 } },
      {
        RunAction: [
          {
            LaunchPlugin: {
              plugin: {
                RunPlugin: {
                  location: { Yinz: "new-agent" },
                  configuration: {
                    caller_terminal_pane_id: "7",
                    [action === "fork_agent" ? "fork_focused" : "duplicate_focused"]: "true",
                    overlay: "true",
                    modal: "true",
                  },
                  initial_cwd: null,
                },
              },
              should_float: true,
              should_open_in_place: false,
              close_replaced_pane: false,
              skip_cache: false,
              cwd: null,
              no_focus: false,
              tab_id: null,
            },
          },
          {},
        ],
      },
    ]);
    for (const missing of [null, { agent: null }]) {
      p.stub("GetPaneInfo", missing);
      await p.event({
        ContextMenuItemSelected: { label: "", item_context: { action }, context: source },
      });
      assertEquals(p.commands.at(-1), { GetPaneInfo: { Terminal: 7 } });
    }
  }
});
test("context-menu removes unavailable Copy and redundant separators", async () => {
  const p = await loadPlugin("./context-menu.ts", {
    replies: { RunCommand: null, SetContextMenu: null, GetPaneInfo: null },
  });
  await p.event({
    ContextMenuRequested: {
      request_id: 9,
      pane_id: { Terminal: 7 },
      items: [
        { separator: true },
        { id: "copy", label: "Copy" },
        { separator: true },
        { id: "close", label: "Close" },
        { separator: true },
      ],
      selected_text: null,
      link: null,
    },
  } as never);
  assertEquals(p.commands.at(-1), {
    SetContextMenu: { request_id: 9, items: [{ id: "close", label: "Close" }] },
  });
});
test("context-menu selection dispatches its chosen link opener", async () => {
  const p = await loadPlugin("./context-menu.ts", { replies: { RunCommand: null } });
  await p.event({ RunCommandResult: [0, [], [], { probe: "xdg-open" }] });
  await p.event({
    ContextMenuItemSelected: { item_context: { action: "open_link", url: "https://example.test" } },
  } as never);
  assertEquals(p.commands.at(-1), {
    RunCommand: [["xdg-open", "https://example.test"], {}, "", {}, 64, null],
  });
});
