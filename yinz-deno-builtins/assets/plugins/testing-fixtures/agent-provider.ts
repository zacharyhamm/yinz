import yinz, { definePlugin, type AgentProvider } from "yinz:plugin";

export const provider: AgentProvider = {
  kind: "fixture-agent",
  executable: "fixture-agent",
  aliases: ["fixture-agent-alias"],
  launch: { model_args: ["--model", "{value}"] },
  input: {
    submit_key: "Ctrl j",
    submit_delay_ms: 175,
    paste_prefix: "Read: ",
    command_prefixes: [":"],
    newline: "\r",
  },
};

export default definePlugin({
  async load() {
    await yinz.waitForPermissions(["RunCommands"]);
    yinz.subscribe(["AgentProvidersChanged"]);
    yinz.agent.registerProvider(provider);
    console.log("fixture agent provider ready");
  },
  update(event) {
    if (typeof event === "object" && "AgentProvidersChanged" in event) {
      for (const registered of event.AgentProvidersChanged) {
        console.log(`fixture observed provider ${registered.kind}`);
      }
    }
  },
});
