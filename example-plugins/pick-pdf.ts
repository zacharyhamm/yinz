// Load as a background plugin. Configure reader with a plugin alias or URL.
import z, { definePlugin } from "yinz:plugin";

export default definePlugin({
  defaults: { reader: "pdf-reader" },
  async load(configuration) {
    await z.waitForPermissions([
      "ReadApplicationState",
      "ReadSessionEnvironmentVariables",
      "MessageAndLaunchOtherPlugins",
    ]);
    const home = z.host.getSessionEnvironmentVariables()["HOME"];
    if (!home) throw new Error("The session has no HOME environment variable");
    z.commandPalette.registerProvider({
      id: "pick-pdf",
      title: "PDF files",
      actions: [
        {
          id: "open",
          label: "Choose a folder, then a PDF",
          async run(context): Promise<void> {
            context.close();
            const folder = await z.pick({ kind: "directory", roots: [home] });
            if (folder === null) return;
            const file = await z.pick({ kind: "file", roots: [folder], extensions: ["pdf"] });
            if (file === null) return;
            z.host.messageToPlugin({
              plugin_url: configuration["reader"] ?? "pdf-reader",
              destination_plugin_id: null,
              plugin_config: {},
              message_name: "open",
              message_payload: file,
              message_args: {},
              new_plugin_args: null,
              floating_pane_coordinates: null,
            });
          },
        },
      ],
    });
  },
});
