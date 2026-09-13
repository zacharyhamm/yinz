// zellij plugin --floating -- file:$PWD/example-plugins/clock.ts
import z, { definePlugin } from "zellij:plugin";
let label = "the time is";
z.on("Key", (key) => {
  if (key.bare_key === "Esc") z.command("CloseSelf");
});
export default definePlugin({
  defaults: { label: "the time is" },
  load(config) {
    label = config["label"];
    z.requestPermission(["ChangeApplicationState"]);
    z.subscribe(["Key"]);
    z.every(1000, () => true);
  },
  render(rows, cols) {
    return `\u001B[1m${label}\u001B[0m ${new Date().toLocaleTimeString()}\n\n[Esc] close   (${rows}x${cols})`;
  },
});
