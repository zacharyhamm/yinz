import {
  AgentConfigSchema,
  type AgentContext,
  AgentObservationSchema,
  AgentResultSchema,
  AgentStateSchema,
  confirmFailureAgent,
  EmptySchema,
  recoverAgent,
  resumeAgent,
  runAgent,
  RunArgsSchema,
  statusAgent,
} from "./_lib/yinz_agent.ts";

export const behavior = {
  type: "@yinz/agent",
  version: "2026.11.04.1",
  globalArguments: AgentConfigSchema,
  resources: {
    state: {
      description:
        "Durable launch intent or rejection, pane and exact submission identity",
      schema: AgentStateSchema,
      lifetime: "infinite",
      garbageCollection: 100,
    },
    result: {
      description: "Confirmed final answer from a visible Yinz agent pane",
      schema: AgentResultSchema,
      lifetime: "infinite",
      garbageCollection: 100,
    },
    observation: {
      description:
        "Latest read-only pane observation, distinct from task completion",
      schema: AgentObservationSchema,
      lifetime: "infinite",
      garbageCollection: 10,
    },
  },
  methods: {
    run: {
      kind: "action",
      description:
        "Spawn a visible agent and submit one guarded prompt; retains uncertain outcomes",
      arguments: RunArgsSchema,
      execute: runAgent,
    },
    recover: {
      kind: "action",
      description:
        "Initialize an unstarted original task or resume its exact submission under the child execution lock",
      arguments: RunArgsSchema,
      execute: recoverAgent,
    },
    confirm_failure: {
      kind: "action",
      description:
        "Verify a terminal or structured unsent failure and fresh idle identity with no background work or pending writes",
      arguments: EmptySchema,
      execute: (_args: Record<string, never>, context: AgentContext) =>
        confirmFailureAgent(context),
    },
    resume: {
      kind: "action",
      description:
        "Retrieve the original submission; rejected lifecycle launches can be discarded, never relaunched here",
      arguments: EmptySchema,
      execute: (_args: Record<string, never>, context: AgentContext) =>
        resumeAgent(context),
    },
    status: {
      kind: "read",
      description:
        "Observe the retained pane without treating idle as task completion",
      arguments: EmptySchema,
      execute: (_args: Record<string, never>, context: AgentContext) =>
        statusAgent(context),
    },
  },
};
