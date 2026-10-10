// Ordinary instances serve only their owning client; shared=true serves all session clients.
// Load with a `url` pointing to a JSON array of {id, label} records.
import { definePlugin, yinz, type PaletteAction, type PaletteView } from "yinz:plugin";

function messageForm(title: string): PaletteView {
  return {
    type: "form",
    title,
    fields: [
      { type: "text", id: "message", label: "Message" },
      { type: "choice", id: "style", label: "Style", options: ["Plain", "Uppercase"] },
    ],
    onSubmit(values) {
      const message = values["message"] ?? "";
      if (!message.trim()) throw new Error("Enter a message");
      return {
        type: "complete",
        notification: "Message formatted",
        view: {
          type: "text",
          title,
          text: values["style"] === "Uppercase" ? message.toUpperCase() : message,
        },
      };
    },
  };
}

function remoteAction(record: unknown): PaletteAction {
  if (
    typeof record !== "object" ||
    record === null ||
    !("id" in record) ||
    !("label" in record) ||
    typeof record.id !== "string" ||
    typeof record.label !== "string"
  ) {
    throw new Error("Actions need string IDs and labels");
  }
  const title = record.label;
  return {
    id: record.id,
    label: title,
    run: () => ({
      type: "actions",
      title,
      actions: [
        { id: "form", label: "Format a message", run: () => messageForm(title) },
        {
          id: "terminal",
          label: "Open terminal",
          run(execution) {
            execution.close();
            yinz.openTerminal(".");
          },
        },
      ],
    }),
  };
}

export default definePlugin({
  defaults: { url: "http://127.0.0.1:8080/actions" },
  async load(configuration) {
    await yinz.waitForPermissions(["ReadApplicationState", "WebAccess", "OpenTerminalsOrPlugins"]);
    yinz.commandPalette.registerProvider({
      id: "remote",
      title: "Remote actions",
      async provideActions(context) {
        const request = yinz.webRequest({
          url: `${configuration["url"]}?query=${encodeURIComponent(context.query)}`,
        });
        context.onCancel(() => {
          request.cancel();
        });
        const response = await request;
        if (response.status !== 200) throw new Error(`HTTP ${response.status}`);
        const records: unknown = JSON.parse(yinz.decodeUtf8(response.body));
        if (!Array.isArray(records)) throw new Error("Expected an array of actions");
        return records.slice(0, 100).map((record: unknown) => remoteAction(record));
      },
    });
  },
  render() {
    return "Remote palette provider ready. Open the command palette to search.";
  },
});
