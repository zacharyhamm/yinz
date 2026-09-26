import { test, loadPlugin, assertEquals } from "zellij:testing";
import { defaultMode } from "./status-bar/default-mode.ts";

test("tab bar starts empty and handles hint replacement, clearing, and full frames", async () => {
  const p = await loadPlugin("./tab-bar.ts", { replies: { GetPluginIds: { hostname: "host" } } });
  assertEquals(await p.render(1, 80), "");
  await p.event({
    TabUpdate: [
      {
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
      },
    ],
  });
  const hint = { 8: { text: "hint-one", indices: [[0, 1000], [], [], [], [2]] } };
  await p.event({ HintText: hint });
  const frame = () => p.render(1, 80);
  assertEquals((await frame()).includes("hint-one"), true);
  await p.event({ HintText: { 8: { text: "hint-two", indices: [] } } });
  await p.advance(5000);
  assertEquals((await frame()).includes("hint-two"), true);
  await p.advance(1000);
  assertEquals((await frame()).includes("hint-two"), false);
  await p.event({ HintText: hint });
  await p.event({ ModeUpdate: { ...defaultMode(), pane_frame_style: "full" } });
  assertEquals((await frame()).includes("hint-one"), false);
  await p.event({ ModeUpdate: { ...defaultMode(), pane_frame_style: "titles" } });
  assertEquals((await frame()).includes("hint-one"), true);
  await p.event("InputReceived");
  assertEquals((await frame()).includes("hint-one"), false);
});
