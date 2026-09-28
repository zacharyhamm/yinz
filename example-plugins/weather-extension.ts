// Install as hocus/weather.ts in the user plugin directory, then plugin_reload it.
import { definePlugin, yinz } from "yinz:plugin";

export default definePlugin({
  async load() {
    await yinz.waitForPermissions(["MessageAndLaunchOtherPlugins"]);
    yinz.hocus.tools({
      forecast: {
        description: "Return a demonstration forecast for a city.",
        parameters: {
          type: "object",
          properties: { city: { type: "string" } },
          required: ["city"],
        },
        safety: "read_only",
        handler(args) {
          if (
            args == null ||
            typeof args !== "object" ||
            Array.isArray(args) ||
            typeof args["city"] !== "string"
          ) {
            throw new Error("city must be a string");
          }
          return { city: args["city"], forecast: "Clear skies (example data)" };
        },
      },
    });
  },
});
