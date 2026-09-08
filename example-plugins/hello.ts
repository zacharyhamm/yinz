import { definePlugin, zellij } from "zellij:plugin";

let greeting = "Starting";

export default definePlugin({
  async load(configuration) {
    await zellij.sleep(100);
    greeting = configuration.greeting ?? "Hello from TypeScript";
    zellij.requestRender();
  },
  render(rows, columns) {
    return `${greeting}\n${columns} columns, ${rows} rows`;
  },
});
