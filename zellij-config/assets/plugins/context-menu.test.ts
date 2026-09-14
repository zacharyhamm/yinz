import { test, loadPlugin, assertEquals } from "zellij:testing";
test("context-menu removes unavailable Copy and redundant separators", async () => {
  const p = await loadPlugin("./context-menu.ts", {
    replies: { RunCommand: null, SetContextMenu: null },
  });
  await p.event({
    ContextMenuRequested: {
      request_id: 9,
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
