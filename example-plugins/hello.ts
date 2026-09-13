import { definePlugin, zellij } from "zellij:plugin";

let greeting = "Starting";

export default definePlugin({
  defaults: {
    greeting: "Hello from TypeScript",
    url: "https://example.com",
  },
  async load(configuration) {
    try {
      await zellij.waitForPermissions(["RunCommands", "WebAccess"]);
      const command = zellij.runCommand({
        command: ["/usr/bin/printf", "%s", configuration["greeting"]],
      });
      const request = zellij.webRequest({ url: configuration["url"] });
      const timeout = zellij.after(5000, () => {
        command.cancel();
        request.cancel();
      });
      try {
        const [output, response] = await Promise.all([command, request]);
        greeting = `${zellij.decodeUtf8(output.stdout)}\nHTTP ${response.status}: ${response.body.length} bytes`;
      } finally {
        timeout.cancel();
        command.cancel();
        request.cancel();
      }
    } catch (error) {
      greeting =
        error instanceof Error && error.name === "AbortError"
          ? "Timed out waiting for results"
          : `Failed: ${String(error)}`;
    }
  },
  render(rows, columns) {
    return `${greeting}\n${columns} columns, ${rows} rows`;
  },
});
