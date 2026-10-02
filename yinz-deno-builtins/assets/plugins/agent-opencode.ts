import yinz, { definePlugin, type AgentProvider } from "yinz:plugin";
import { hookSource } from "./agents/opencode-hooks.ts";

export const agentProvider: AgentProvider = {
  kind: "opencode",
  executable: "opencode",
  launch: {
    model_args: ["--model", "{value}"],
    prompt_args: ["--prompt", "{value}"],
    resume_args: ["--session", "{value}"],
    resume_last_args: ["--continue"],
    permissions: { auto: ["--auto"], skip: ["--auto"] },
    permission_descriptions: {
      auto: "Auto-approves; explicit denials stay",
      skip: "Auto-approves; explicit denials stay",
    },
  },
  restart: {
    values: [
      "-m",
      "--model",
      "--agent",
      "--log-level",
      "--port",
      "--hostname",
      "--mdns-domain",
      "--replay-limit",
    ],
    switches: ["--auto", "--print-logs", "--pure", "--mdns", "--mini", "--no-replay"],
    inline_values: ["--cors"],
    remove_values: ["--prompt", "-s", "--session"],
    remove_switches: ["-c", "--continue", "--fork"],
    forbidden_commands: [
      "completion",
      "acp",
      "mcp",
      "attach",
      "run",
      "debug",
      "providers",
      "auth",
      "agent",
      "upgrade",
      "uninstall",
      "serve",
      "web",
      "models",
      "stats",
      "export",
      "import",
      "github",
      "pr",
      "session",
      "plugin",
      "plug",
      "db",
      "help",
    ],
    keep_positionals: true,
    max_positionals: 1,
  },
  // OpenCode reports status reliably; its hooks do not yet correlate submitted prompts
  // with completion. Omit input until that contract can be verified against the agent.
  hooks: [
    {
      base: "config",
      path: "opencode/plugin/hocus.js",
      marker: "hocus-opencode-plugin",
      content: hookSource,
    },
  ],
};

export default definePlugin({
  load() {
    yinz.agent.registerProvider(agentProvider);
  },
});
