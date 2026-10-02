import { test, loadPlugin, assert, assertEquals } from "yinz:testing";
import { agentProvider } from "./agent-opencode.ts";

test("OpenCode registers its TypeScript provider and hook adapter", async () => {
  const plugin = await loadPlugin("./agent-opencode.ts", {
    replies: { RegisterAgentProvider: [agentProvider] },
  });
  assertEquals(plugin.commands, [{ RegisterAgentProvider: agentProvider }]);
  assertEquals(agentProvider.launch?.prompt_args, ["--prompt", "{value}"]);
  assertEquals(agentProvider.launch?.resume_args, ["--session", "{value}"]);
  assertEquals(agentProvider.launch?.permissions?.skip, ["--auto"]);
  assert(!agentProvider.input, "status hooks alone do not establish prompt support");
  const hook = agentProvider.hooks?.[0];
  assert(hook?.content.includes("JSON.stringify(toHookEvent(payload))"));
  assert(hook?.content.includes("AskUserQuestion"));
  assert(hook?.content.includes("__TOKEN_ENV__"));
});
