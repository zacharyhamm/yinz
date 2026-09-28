// Own a bounded model/tool loop. Hocus-agentd serves stateless completion requests.
import z, { definePlugin, type CompletionMessage, type PaneManifest } from "yinz:plugin";
let asking = false,
  panes: PaneManifest["panes"] = {};
const lines = ["press <Enter> to ask, or q to quit"];
function say(line: string): void {
  lines.push(line);
  z.requestRender();
}
// oxlint-disable-next-line eslint/max-lines-per-function -- This lifecycle handler keeps its ordered protocol transitions together.
async function ask(): Promise<void> {
  if (asking) return;
  asking = true;
  say("asking...");
  const messages: CompletionMessage[] = [
    { role: "user", content: "What am I running in this session right now?" },
  ];
  try {
    for (let i = 0; i < 5; i++) {
      // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
      const reply = await z.hocus.complete({
        system: "You answer about this terminal session. Be brief.",
        messages,
        timeoutMs: 60000,
        tools: [
          {
            type: "function",
            function: {
              name: "count_panes",
              description: "How many panes are open and what is running in them.",
              parameters: { type: "object", additionalProperties: false },
            },
          },
        ],
      });
      if (!reply.toolCalls?.length) {
        say(reply.content ?? "");
        break;
      }
      messages.push({
        role: "assistant",
        content: reply.content ?? "",
        tool_calls: reply.toolCalls,
      });
      for (const call of reply.toolCalls) {
        say(`· ${call.name} ${call.arguments}`);
        const running = Object.values(panes)
          .flat()
          .map((pane) => pane.title ?? "?");
        messages.push({
          role: "tool",
          tool_call_id: call.id,
          content:
            call.name === "count_panes"
              ? `${running.length} panes: ${running.join(", ")}`
              : `no such tool: ${call.name}`,
        });
      }
    }
  } catch (error) {
    say(String(error));
  } finally {
    asking = false;
  }
}
z.on("PaneUpdate", (manifest) => {
  panes = manifest.panes;
});
z.on("Key", (key) => {
  if (key.bare_key === "Enter")
    void ask().catch((error: unknown) => {
      z.log(String(error));
    });
  else if (
    key.bare_key === "Esc" ||
    (typeof key.bare_key === "object" && "Char" in key.bare_key && key.bare_key.Char === "q")
  )
    z.command("CloseSelf");
});
export default definePlugin({
  load() {
    z.requestPermission([
      "MessageAndLaunchOtherPlugins",
      "ReadApplicationState",
      "ChangeApplicationState",
    ]);
    z.subscribe(["PaneUpdate", "Key"]);
  },
  render: () => lines.join("\n"),
});
