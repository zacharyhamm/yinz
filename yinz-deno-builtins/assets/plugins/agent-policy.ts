// Headless hook: log each firing and focus a waiting agent when its toast is clicked.
// Bind with: yinz action set-hook agent_waiting yinz:agent-policy --write
// Builtins have permissions; file copies must request ChangeApplicationState in load().
import yinz, { definePlugin } from "yinz:plugin";

yinz.on("Hook", ({ point, payload }) => {
  yinz.log(`hook ${point} ${JSON.stringify(payload)}`);
  if (point === "agent_waiting" && "agent" in payload) {
    // oxlint-disable-next-line typescript/prefer-nullish-coalescing -- An empty wire string needs the same display fallback as an absent value.
    const message = `${payload.agent.kind || "agent"} in pane ${payload.pane_id}: ${payload.agent.detail || "is waiting for you"}`;
    yinz.command("Notify", [message, { Terminal: payload.pane_id }, null]);
  }
});

export default definePlugin({
  load() {
    yinz.subscribe(["Hook"]);
  },
});
