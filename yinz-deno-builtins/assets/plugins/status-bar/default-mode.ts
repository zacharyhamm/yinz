import type { ModeInfo, StyleDeclaration } from "yinz:plugin";

// Match ModeInfo::default before the host sends its first mode/theme update.
const declaration = (
  base: number,
  background: number,
  e0: number,
  e1: number,
  e2: number,
  e3: number,
): StyleDeclaration => ({
  base: { EightBit: base },
  background: { EightBit: background },
  emphasis_0: { EightBit: e0 },
  emphasis_1: { EightBit: e1 },
  emphasis_2: { EightBit: e2 },
  emphasis_3: { EightBit: e3 },
});
// oxlint-disable-next-line eslint/max-lines-per-function -- Keep the initial wire value aligned with ModeInfo::default, including its complete palette.
export function defaultMode(): ModeInfo {
  return {
    mode: "Normal",
    base_mode: null,
    keybinds: [],
    style: {
      rounded_corners: false,
      hide_session_name: false,
      colors: {
        pane_surround: null,
        text_unselected: declaration(245, 238, 166, 51, 154, 201),
        text_selected: declaration(245, 238, 166, 51, 154, 201),
        ribbon_unselected: declaration(16, 238, 124, 255, 45, 201),
        ribbon_selected: declaration(16, 154, 124, 166, 201, 45),
        exit_code_success: declaration(154, 238, 51, 16, 201, 45),
        exit_code_error: declaration(124, 238, 226, 136, 245, 99),
        frame_selected: declaration(154, 238, 166, 51, 201, 215),
        frame_highlight: declaration(166, 154, 201, 99, 154, 154),
        table_title: declaration(154, 238, 166, 51, 154, 201),
        table_cell_unselected: declaration(245, 238, 166, 51, 154, 201),
        table_cell_selected: declaration(154, 238, 166, 51, 124, 201),
        list_unselected: declaration(245, 238, 166, 51, 154, 201),
        list_selected: declaration(154, 238, 166, 51, 124, 201),
        multiplayer_user_colors: {
          player_1: { EightBit: 201 },
          player_2: { EightBit: 45 },
          player_3: { EightBit: 99 },
          player_4: { EightBit: 226 },
          player_5: { EightBit: 51 },
          player_6: { EightBit: 136 },
          player_7: { EightBit: 124 },
          player_8: { EightBit: 245 },
          player_9: { EightBit: 207 },
          player_10: { EightBit: 215 },
        },
        frame_unselected: null,
      },
    },
    capabilities: { arrow_fonts: true, kitty_graphics: false },
    session_name: null,
    editor: null,
    shell: null,
    currently_marking_pane_group: null,
    pane_frame_style: null,
    session_dimmed: null,
    session_ancestry: [],
    host_fullscreen: null,
    nested_ascend_keys: [],
    session_ascended: null,
    nested_descend_keys: [],
  };
}
