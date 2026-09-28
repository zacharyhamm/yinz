import { definePlugin, yinz } from "yinz:plugin";

let greeting = "Starting";

export default definePlugin({
  defaults: {
    greeting: "Hello from TypeScript",
    url: "https://example.com",
  },
  async load(configuration) {
    try {
      await yinz.waitForPermissions(["RunCommands", "WebAccess"]);
      const command = yinz.runCommand({
        command: ["/usr/bin/printf", "%s", configuration["greeting"]],
      });
      const request = yinz.webRequest({ url: configuration["url"] });
      const timeout = yinz.after(5000, () => {
        command.cancel();
        request.cancel();
      });
      try {
        const [output, response] = await Promise.all([command, request]);
        greeting = `${yinz.decodeUtf8(output.stdout)}\nHTTP ${response.status}: ${response.body.length} bytes`;
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
