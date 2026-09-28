// yinz plugin --floating -- file:$PWD/example-plugins/keybindings.ts
// Fallback keybindings owned by the plugin: Ctrl g runs a handler here, Ctrl n opens a pane.
import z, { definePlugin } from "yinz:plugin";
let presses = 0,
  state = "waiting for the Reconfigure grant";
export default definePlugin({
  async load() {
    await z.waitForPermissions(["Reconfigure"]);
    z.registerDefaultKeybindings([
      {
        mode: "normal",
        key: "Ctrl g",
        handler(): boolean {
          presses += 1;
          return true;
        },
      },
      { mode: "normal", key: "Ctrl n", actions: [z.action("NewPane")] },
    ]);
    state = "registered";
  },
  render() {
    return `${state}\nCtrl g pressed ${presses} time(s); Ctrl n opens a pane`;
  },
});
